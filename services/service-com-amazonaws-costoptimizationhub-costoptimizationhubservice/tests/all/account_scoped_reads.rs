// SPDX-License-Identifier: CC0-1.0

//! Which ACCOUNTS a Cost Optimization Hub read answers for, driven end-to-end
//! over the wire (`awsJson1_0`) through the same multi-service router the
//! emulator boots.
//!
//! The rule under test: a read answers for the accounts its request NAMES —
//! `ListEnrollmentStatuses`' `accountId`
//! (`smithy_query(query="doc", shape="ListEnrollmentStatusesRequest",
//! member="accountId")`: "The account ID of a member account in the
//! organization"), the recommendation readers' `filter.accountIds` — and,
//! naming none, for the calling account alone, widened to every account of the
//! organization where the request asks for the organization
//! (`includeOrganizationInfo`). The accounts the answer is scoped to are
//! therefore the same ones the operation's permission gate judged: a request
//! the gate admits as a read of a named account answers that account's data
//! rather than an empty list.
//!
//! The gate itself — who may reach past their own account — is
//! `management_account_gating`'s; every call here is made by the
//! organization's MANAGEMENT account, whose reads the gate admits, so what is
//! asserted is the scoping that follows it.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice::state::Enrollment;

use crate::cost_optimization_hub::{
    call, ids, management_account_router_with_store, ok, router, Store, ANALYSIS_WAIT,
};

/// Member accounts of the organization the store records. Twelve digits, which
/// is what the `AccountId` shape's `^[0-9]{12}$` pattern admits, and none of
/// them the caller.
const MEMBER_ACTIVE: &str = "111122223333";
const MEMBER_INACTIVE: &str = "444455556666";
const MEMBER_FAILED: &str = "777788889999";

/// An account the organization has no record for at all, so a read that names
/// it has nothing to report.
const UNKNOWN_ACCOUNT: &str = "222233334444";

/// Record `account`'s enrollment in the store, the way an Organizations
/// control plane this emulator does not have would have left it.
///
/// `recommendations_due` decides whether the analysis the enrollment started
/// is already due, which is what makes the account's catalog rows readable.
fn record_enrollment(store: &Store, account: &str, status: &str, recommendations_due: bool) {
    let now = std::time::SystemTime::now();
    let timestamp = aws_smithy_types::DateTime::from(now);
    store.lock().enrollments.insert(
        account.to_string(),
        Enrollment {
            account_id: account.to_string(),
            status: status.to_string(),
            include_member_accounts: None,
            created_timestamp: timestamp,
            last_updated_timestamp: timestamp,
            recommendations_due_at: recommendations_due.then_some(now),
        },
    );
}

/// Give `account` its own rows in the catalog — copies of the first `count`
/// rows, under new ids — and answer the ids created, in catalog order.
fn catalog_rows_for(store: &Store, account: &str, count: usize) -> Vec<String> {
    let mut store = store.lock();
    let sources: Vec<String> = store
        .recommendation_order
        .iter()
        .take(count)
        .cloned()
        .collect();
    let mut created = Vec::new();
    for source in sources {
        let mut row = store
            .recommendations
            .get(&source)
            .expect("a catalog row for every ordered id")
            .clone();
        row.account_id = account.to_string();
        row.recommendation_id = format!("{account}-{source}");
        row.resource_id = format!("{account}-{}", row.resource_id);
        created.push(row.recommendation_id.clone());
        store.recommendation_order.push(row.recommendation_id.clone());
        store
            .recommendations
            .insert(row.recommendation_id.clone(), row);
    }
    created
}

/// One enrollment record as an answer carries it: the account and its status.
type Record = (String, String);

/// One row of the enrollment table: the case it names, the request it sends,
/// and the records the answer must carry.
type EnrollmentCase = (String, Value, Vec<Record>);

/// The `(accountId, status)` pairs a `ListEnrollmentStatuses` answer carries,
/// in the order it carries them.
fn listed(payload: &Value) -> Vec<Record> {
    payload["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|item| {
            (
                item["accountId"].as_str().unwrap_or_default().to_string(),
                item["status"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// `ListEnrollmentStatuses` answers for the accounts the request names.
///
/// The table spans what the rule generalises over: whether an account is
/// named, which one (the caller, each of three members whose records differ in
/// status, one the organization has no record for), and whether the
/// organization is asked for beside it.
#[tokio::test]
async fn an_enrollment_read_answers_for_the_accounts_the_request_names() {
    let (router, store) = management_account_router_with_store();
    let caller = engine::emulator_account_id();
    record_enrollment(&store, MEMBER_ACTIVE, "Active", true);
    record_enrollment(&store, MEMBER_INACTIVE, "Inactive", false);
    record_enrollment(&store, MEMBER_FAILED, "Failed", false);

    // The whole organization, in the order the answer is paged in.
    let organization: Vec<Record> = {
        let mut all = vec![
            (caller.to_string(), "Inactive".to_string()),
            (MEMBER_ACTIVE.to_string(), "Active".to_string()),
            (MEMBER_INACTIVE.to_string(), "Inactive".to_string()),
            (MEMBER_FAILED.to_string(), "Failed".to_string()),
        ];
        all.sort();
        all
    };

    // (case, request, the records the answer must carry)
    let cases: Vec<EnrollmentCase> = vec![
        (
            "no account named".to_string(),
            json!({"includeOrganizationInfo": false}),
            vec![(caller.to_string(), "Inactive".to_string())],
        ),
        (
            "the caller named".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": caller}),
            vec![(caller.to_string(), "Inactive".to_string())],
        ),
        (
            "an enrolled member named".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": MEMBER_ACTIVE}),
            vec![(MEMBER_ACTIVE.to_string(), "Active".to_string())],
        ),
        (
            "a member that opted out named".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": MEMBER_INACTIVE}),
            vec![(MEMBER_INACTIVE.to_string(), "Inactive".to_string())],
        ),
        (
            "a member whose enrollment failed named".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": MEMBER_FAILED}),
            vec![(MEMBER_FAILED.to_string(), "Failed".to_string())],
        ),
        (
            "a member named, with the organization asked for too".to_string(),
            json!({"includeOrganizationInfo": true, "accountId": MEMBER_ACTIVE}),
            vec![(MEMBER_ACTIVE.to_string(), "Active".to_string())],
        ),
        (
            "an account the organization has no record for".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": UNKNOWN_ACCOUNT}),
            Vec::new(),
        ),
        (
            "the organization, no account named".to_string(),
            json!({"includeOrganizationInfo": true}),
            organization.clone(),
        ),
    ];

    for (case, request, expected) in &cases {
        let payload = ok(&router, "ListEnrollmentStatuses", request).await;
        let answered = listed(&payload);
        assert_eq!(
            &answered, expected,
            "case {case:?}: expected {expected:?}, got {answered:?} in {payload}"
        );
        assert!(
            payload["nextToken"].is_null(),
            "case {case:?}: the whole answer fits one page, got {payload}"
        );
    }

    // Reading twice answers the same thing: scoping a read changes no record.
    let repeated = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": true}),
    )
    .await;
    assert_eq!(
        listed(&repeated),
        organization,
        "case repeated read: the organization's records changed between two reads, got {repeated}"
    );
}

/// The organization view pages over the accounts it is scoped to, and a page
/// resumes at the account the previous page stopped at.
#[tokio::test]
async fn the_organization_view_pages_over_the_accounts_it_is_scoped_to() {
    let (router, store) = management_account_router_with_store();
    record_enrollment(&store, MEMBER_ACTIVE, "Active", true);
    record_enrollment(&store, MEMBER_INACTIVE, "Inactive", false);
    record_enrollment(&store, MEMBER_FAILED, "Failed", false);

    let whole = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": true}),
    )
    .await;
    let expected = listed(&whole);
    assert_eq!(
        expected.len(),
        4,
        "case setup: expected the caller and three members, got {whole}"
    );

    // Every page size from one row to one past the whole answer.
    for size in 1..=expected.len() + 1 {
        let case = format!("maxResults {size}");
        let mut seen: Vec<Record> = Vec::new();
        let mut request = json!({"includeOrganizationInfo": true, "maxResults": size});
        for _ in 0..expected.len() + 2 {
            let payload = ok(&router, "ListEnrollmentStatuses", &request).await;
            let page = listed(&payload);
            assert!(
                page.len() <= size,
                "case {case}: a page carried more than {size} records, got {payload}"
            );
            seen.extend(page);
            match payload["nextToken"].as_str() {
                None => break,
                Some(token) => {
                    request = json!({
                        "includeOrganizationInfo": true,
                        "maxResults": size,
                        "nextToken": token,
                    });
                }
            }
        }
        assert_eq!(
            seen, expected,
            "case {case}: expected the whole organization across pages, got {seen:?}"
        );
    }
}

/// The recommendation readers scope to the same accounts: a `filter.accountIds`
/// the gate admits answers those accounts' rows.
///
/// The table spans how many accounts the filter names, whose they are, and
/// whether the named account's recommendations are readable at all —
/// enrollment is still judged per account, so a member that has not opted in
/// contributes nothing even when it owns rows in the catalog.
#[tokio::test]
async fn a_recommendation_read_answers_for_the_accounts_the_filter_names() {
    let (router, store) = management_account_router_with_store();
    let caller = engine::emulator_account_id();

    // The caller enrolls over the wire, as a client would; the members are
    // recorded in the store, because no operation enrolls another account.
    ok(
        &router,
        "UpdateEnrollmentStatus",
        &json!({"status": "Active", "includeMemberAccounts": true}),
    )
    .await;
    record_enrollment(&store, MEMBER_ACTIVE, "Active", true);
    record_enrollment(&store, MEMBER_INACTIVE, "Inactive", false);
    let member_rows = catalog_rows_for(&store, MEMBER_ACTIVE, 2);
    let opted_out_rows = catalog_rows_for(&store, MEMBER_INACTIVE, 2);
    assert!(
        !member_rows.is_empty() && !opted_out_rows.is_empty(),
        "case setup: expected the members to own catalog rows, got {member_rows:?} and \
         {opted_out_rows:?}"
    );
    // The caller's own analysis has to have come due before its rows read.
    std::thread::sleep(ANALYSIS_WAIT);

    let caller_rows = ids(&ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true, "filter": {"accountIds": [caller]}}),
    )
    .await);
    assert!(
        !caller_rows.is_empty(),
        "case setup: expected the enrolled caller's own rows to be readable"
    );

    // (case, the accountIds the filter names, the ids the answer must carry)
    let both: Vec<String> = caller_rows
        .iter()
        .cloned()
        .chain(member_rows.iter().cloned())
        .collect();
    let cases: Vec<(String, Value, Vec<String>)> = vec![
        (
            "the caller alone".to_string(),
            json!([caller]),
            caller_rows.clone(),
        ),
        (
            "one enrolled member".to_string(),
            json!([MEMBER_ACTIVE]),
            member_rows.clone(),
        ),
        (
            "the caller and a member".to_string(),
            json!([caller, MEMBER_ACTIVE]),
            both,
        ),
        (
            "a member that opted out".to_string(),
            json!([MEMBER_INACTIVE]),
            Vec::new(),
        ),
        (
            "an account the organization has no record for".to_string(),
            json!([UNKNOWN_ACCOUNT]),
            Vec::new(),
        ),
        (
            "a member named twice".to_string(),
            json!([MEMBER_ACTIVE, MEMBER_ACTIVE]),
            member_rows.clone(),
        ),
    ];

    for (case, account_ids, expected) in &cases {
        let payload = ok(
            &router,
            "ListRecommendations",
            &json!({
                "includeAllRecommendations": true,
                "filter": {"accountIds": account_ids},
            }),
        )
        .await;
        let answered = ids(&payload);
        assert_eq!(
            &answered, expected,
            "case {case:?}: expected {expected:?}, got {answered:?} in {payload}"
        );
        for item in payload["items"].as_array().expect("items is a list") {
            let owner = item["accountId"].as_str().unwrap_or_default();
            assert!(
                account_ids
                    .as_array()
                    .expect("the filter names a list of accounts")
                    .iter()
                    .any(|named| named.as_str() == Some(owner)),
                "case {case:?}: a row of {owner} was answered for {account_ids}, got {payload}"
            );
        }
    }

    // The detail view agrees: a row the list answered for a member is the
    // member's row, and its summary reports the same accounts.
    let detail = ok(
        &router,
        "ListRecommendationSummaries",
        &json!({"groupBy": "AccountId", "filter": {"accountIds": [MEMBER_ACTIVE]}}),
    )
    .await;
    let groups: Vec<String> = detail["items"]
        .as_array()
        .expect("items is a list")
        .iter()
        .map(|item| item["group"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        groups,
        vec![MEMBER_ACTIVE.to_string()],
        "case summary of one member: expected the member's group, got {detail}"
    );
}

/// Nothing above widens who may reach past their own account: scoping a read
/// to a named account is what the management account's admitted read answers,
/// and a caller the gate refuses still gets 403 before any scoping runs.
#[tokio::test]
async fn naming_an_account_is_still_refused_to_a_non_management_account() {
    let router = router();
    for account in [MEMBER_ACTIVE, MEMBER_INACTIVE, UNKNOWN_ACCOUNT] {
        let (status, payload) = call(
            &router,
            "ListEnrollmentStatuses",
            &json!({"includeOrganizationInfo": false, "accountId": account}),
        )
        .await;
        assert_eq!(
            status, 403,
            "case {account}: expected HTTP 403, got {status} with {payload}"
        );
    }
}
