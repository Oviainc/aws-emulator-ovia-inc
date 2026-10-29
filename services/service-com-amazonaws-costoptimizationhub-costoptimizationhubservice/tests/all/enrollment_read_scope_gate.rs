// SPDX-License-Identifier: CC0-1.0

//! The SCOPE a `ListEnrollmentStatuses` request resolves to, and the permission
//! that scope needs, driven end-to-end over the wire (`awsJson1_0`) through the
//! same multi-service router the emulator boots.
//!
//! The rule under test: the accounts a read addresses are decided once — the
//! account the request NAMES, else the calling account, widened to the
//! organization only where no account is named
//! (`smithy_query(query="doc", shape="ListEnrollmentStatusesRequest",
//! member="accountId")`: "The account ID of a member account in the
//! organization"; `member="includeOrganizationInfo"`: "Indicates whether to
//! return the enrollment status for the organization") — and the read is
//! refused as the organization's view, which only the organization's management
//! account has, exactly when that scope reaches an account other than the
//! caller's. So a request confined to the caller's own record is served
//! whatever else it sets, and a request that reaches past it is refused to a
//! member account and served to the management account, with the records of the
//! accounts the scope names.
//!
//! `management_account_gating` owns who may reach past their own account and
//! `account_scoped_reads` the records an admitted read answers; this suite is
//! about the two being decided from ONE scope, so it drives every combination
//! of the two members through both callers.

use serde_json::{json, Value};

use crate::cost_optimization_hub::{
    call, error_code, listed, management_account_router_with_store, ok, record_enrollment,
    router_with_store, Record, Store,
};

/// Member accounts of the organization the store records: twelve digits, which
/// is what the `AccountId` shape's `^[0-9]{12}$` pattern admits, and neither of
/// them the caller.
const MEMBER_ACTIVE: &str = "111122223333";
const MEMBER_INACTIVE: &str = "444455556666";

/// An account the store holds no enrollment record for at all, so a read scoped
/// to it has nothing to report.
const UNKNOWN_ACCOUNT: &str = "222233334444";

/// One row of the table: the request's two scope members (and a page size, to
/// send the combination beside another member), with what each caller must be
/// answered.
struct Case {
    /// What the case is called in every assertion of this row.
    name: &'static str,
    /// The `accountId` the request names, or `None` where it names none.
    account_id: Option<&'static str>,
    /// The `includeOrganizationInfo` the request sends, or `None` where the
    /// member is omitted and the model's `false` default applies.
    include_organization_info: Option<bool>,
    /// The `maxResults` the request sends, if any.
    max_results: Option<u32>,
    /// Whether the scope reaches past the caller's own account — so a member
    /// account is refused this read and only the management account is served
    /// it.
    reaches_past_caller: bool,
    /// The records an admitted read must answer, as `(accountId, status)`.
    served: &'static [(&'static str, &'static str)],
}

/// The request one row sends.
fn request(case: &Case, caller: &str) -> Value {
    let mut request = json!({});
    if let Some(account_id) = case.account_id {
        let named = if account_id == CALLER { caller } else { account_id };
        request["accountId"] = json!(named);
    }
    if let Some(include) = case.include_organization_info {
        request["includeOrganizationInfo"] = json!(include);
    }
    if let Some(max_results) = case.max_results {
        request["maxResults"] = json!(max_results);
    }
    request
}

/// Stands in for the calling account in a row, whose id is the emulator's and
/// so not a constant this file can write.
const CALLER: &str = "<caller>";

/// The records a row expects, with [`CALLER`] resolved.
fn expected(case: &Case, caller: &str) -> Vec<Record> {
    let mut records: Vec<Record> = case
        .served
        .iter()
        .map(|(account, status)| {
            let account = if *account == CALLER { caller } else { account };
            (account.to_string(), (*status).to_string())
        })
        .collect();
    records.sort();
    records
}

/// Every combination of the two members that decide the scope, spanning whether
/// an account is named, which one (the caller, an enrolled member, a member
/// that opted out, one the store holds no record for), whether the organization
/// is asked for beside it, and the member omitted entirely.
fn cases() -> Vec<Case> {
    vec![
        Case {
            name: "no account named, the organization member omitted",
            account_id: None,
            include_organization_info: None,
            max_results: None,
            reaches_past_caller: false,
            served: &[(CALLER, "Inactive")],
        },
        Case {
            name: "no account named, the organization not asked for",
            account_id: None,
            include_organization_info: Some(false),
            max_results: None,
            reaches_past_caller: false,
            served: &[(CALLER, "Inactive")],
        },
        Case {
            name: "the organization asked for, no account named",
            account_id: None,
            include_organization_info: Some(true),
            max_results: None,
            reaches_past_caller: true,
            served: &[
                (CALLER, "Inactive"),
                (MEMBER_ACTIVE, "Active"),
                (MEMBER_INACTIVE, "Inactive"),
            ],
        },
        Case {
            name: "the caller named, the organization member omitted",
            account_id: Some(CALLER),
            include_organization_info: None,
            max_results: None,
            reaches_past_caller: false,
            served: &[(CALLER, "Inactive")],
        },
        Case {
            name: "the caller named, the organization not asked for",
            account_id: Some(CALLER),
            include_organization_info: Some(false),
            max_results: None,
            reaches_past_caller: false,
            served: &[(CALLER, "Inactive")],
        },
        Case {
            name: "the caller named, with the organization asked for too",
            account_id: Some(CALLER),
            include_organization_info: Some(true),
            max_results: None,
            reaches_past_caller: false,
            served: &[(CALLER, "Inactive")],
        },
        Case {
            name: "the caller named and the organization asked for, paged",
            account_id: Some(CALLER),
            include_organization_info: Some(true),
            max_results: Some(10),
            reaches_past_caller: false,
            served: &[(CALLER, "Inactive")],
        },
        Case {
            name: "an enrolled member named, the organization not asked for",
            account_id: Some(MEMBER_ACTIVE),
            include_organization_info: Some(false),
            max_results: None,
            reaches_past_caller: true,
            served: &[(MEMBER_ACTIVE, "Active")],
        },
        Case {
            name: "an enrolled member named, with the organization asked for too",
            account_id: Some(MEMBER_ACTIVE),
            include_organization_info: Some(true),
            max_results: None,
            reaches_past_caller: true,
            served: &[(MEMBER_ACTIVE, "Active")],
        },
        Case {
            name: "a member that opted out named, the organization member omitted",
            account_id: Some(MEMBER_INACTIVE),
            include_organization_info: None,
            max_results: None,
            reaches_past_caller: true,
            served: &[(MEMBER_INACTIVE, "Inactive")],
        },
        Case {
            name: "an account the store holds no record for, with the organization asked for too",
            account_id: Some(UNKNOWN_ACCOUNT),
            include_organization_info: Some(true),
            max_results: None,
            reaches_past_caller: true,
            served: &[],
        },
    ]
}

/// Record the organization's member accounts, which no operation of this
/// emulator creates.
fn record_members(store: &Store) {
    record_enrollment(store, MEMBER_ACTIVE, "Active", true);
    record_enrollment(store, MEMBER_INACTIVE, "Inactive", false);
}

/// A caller that is not the organization's management account is refused
/// exactly the requests whose scope reaches past its own account, and served
/// its own record for every request confined to it — including one that names
/// its own account and asks for the organization in the same call.
#[tokio::test]
async fn a_member_account_is_refused_exactly_the_reads_that_reach_past_it() {
    let (router, store) = router_with_store();
    // The member records exist, so a refusal below is the caller's permission
    // and not an empty store.
    record_members(&store);
    let caller = engine::emulator_account_id();

    for case in cases() {
        let name = case.name;
        let body = request(&case, caller);
        let (status, payload) = call(&router, "ListEnrollmentStatuses", &body).await;
        if case.reaches_past_caller {
            assert_eq!(
                status, 403,
                "case {name:?}: expected HTTP 403 for {body}, got {status} with {payload}"
            );
            let code = error_code(&payload);
            assert_eq!(
                code, "AccessDeniedException",
                "case {name:?}: expected AccessDeniedException, got {code:?} in {payload}"
            );
            assert!(
                payload["message"]
                    .as_str()
                    .is_some_and(|message| !message.is_empty()),
                "case {name:?}: expected a message naming the rule, got {payload}"
            );
        } else {
            assert_eq!(
                status, 200,
                "case {name:?}: expected HTTP 200 for {body}, got {status} with {payload}"
            );
            let answered = listed(&payload);
            let expected = expected(&case, caller);
            assert_eq!(
                answered, expected,
                "case {name:?}: expected {expected:?}, got {answered:?} in {payload}"
            );
            assert!(
                payload["nextToken"].is_null(),
                "case {name:?}: the whole answer fits one page, got {payload}"
            );
        }
    }
}

/// The organization's management account is served every one of those
/// combinations, with the records of the accounts the scope names — the same
/// scope the refusal above is decided from.
#[tokio::test]
async fn the_management_account_is_served_the_records_the_scope_names() {
    let (router, store) = management_account_router_with_store();
    record_members(&store);
    let caller = engine::emulator_account_id();

    for case in cases() {
        let name = case.name;
        let body = request(&case, caller);
        let payload = ok(&router, "ListEnrollmentStatuses", &body).await;
        let answered = listed(&payload);
        let expected = expected(&case, caller);
        assert_eq!(
            answered, expected,
            "case {name:?}: expected {expected:?} for {body}, got {answered:?} in {payload}"
        );
    }
}

/// Naming the caller's own account answers the caller's own record whatever
/// `includeOrganizationInfo` is set to, and reading it changes nothing a later
/// reader sees.
#[tokio::test]
async fn naming_the_caller_reads_one_record_whatever_the_organization_member_says() {
    let (router, store) = router_with_store();
    record_members(&store);
    let caller = engine::emulator_account_id();

    let plain = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    for include in [false, true] {
        let case = format!("includeOrganizationInfo {include}");
        let named = ok(
            &router,
            "ListEnrollmentStatuses",
            &json!({"includeOrganizationInfo": include, "accountId": caller}),
        )
        .await;
        assert_eq!(
            named, plain,
            "case {case}: expected the same answer as the unnamed read {plain}, got {named}"
        );
    }

    let again = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    assert_eq!(
        again, plain,
        "case repeated read: the enrollment record changed between two reads, got {again}"
    );
}

