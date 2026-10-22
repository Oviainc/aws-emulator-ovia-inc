// SPDX-License-Identifier: CC0-1.0

//! The organization gate, driven end-to-end over the wire (`awsJson1_0`)
//! through the same multi-service router the emulator boots.
//!
//! The rule under test: a Cost Optimization Hub feature whose scope is the
//! organization's MEMBER accounts belongs to the organization's management
//! account alone — writing the member-account discount-visibility preference,
//! the savings-estimation mode that depends on it, and reading the enrollment
//! or the recommendations of accounts other than the caller's. This emulator
//! records no organization and so no management account
//! (`state::organization_management_account_id`), so every such request is
//! refused with the operation's own modelled error — HTTP 400
//! `ValidationException` for a preference the account may not write, HTTP 403
//! `AccessDeniedException` for a view it does not have
//! (`smithy_query(query="operation", operation="UpdatePreferences,ListRecommendations,ListEnrollmentStatuses")`)
//! — nothing is stored, and the same request without the organization-scoped
//! member is served.
//!
//! Each rule is asserted over a table that spans the member's values, the
//! members it is sent beside, and the operations that declare it; the request
//! the issue recorded is one row of each.

use serde_json::{json, Value};
use service_com_amazonaws_costoptimizationhub_costoptimizationhubservice as cost_optimization_hub;
use smithy_server_runtime::MultiServiceRouter;

use crate::cost_optimization_hub::{call, enrolled_router, error_code, ok, router};

/// The model's own value set for the management-account-only preference, so the
/// table spans every value the member can take rather than the one the issue
/// cited (`smithy_query(query="shape", shape="MemberAccountDiscountVisibility")`).
use cost_optimization_hub::ops::types::shared_types::MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES;

/// Assert one refused call: the status, the wire error code, and that the
/// answer carries a non-empty message.
async fn refused(
    router: &MultiServiceRouter,
    case: &str,
    operation: &str,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
) {
    let (status, payload) = call(router, operation, body).await;
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected HTTP {expected_status}, got {status} with {payload}"
    );
    let code = error_code(&payload);
    assert_eq!(
        code, expected_code,
        "case {case:?}: expected {expected_code}, got {code:?} in {payload}"
    );
    assert!(
        payload["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "case {case:?}: expected a message naming the rule, got {payload}"
    );
}

/// `memberAccountDiscountVisibility` is the management account's to write, so
/// every request that sets it is refused with 400 `ValidationException`,
/// whichever value it sets and whatever else it sends — and the preference
/// record keeps what it held, so `GetPreferences` never reports the member.
#[tokio::test]
async fn member_account_discount_visibility_is_refused_to_a_non_management_account() {
    let router = router();

    // A preference the account MAY set, so the record under test is non-empty
    // and a refusal that wrote something would show.
    let before = ok(
        &router,
        "UpdatePreferences",
        &json!({
            "savingsEstimationMode": "BeforeDiscounts",
            "preferredCommitment": {"term": "OneYear", "paymentOption": "NoUpfront"},
        }),
    )
    .await;

    // (case, request) — every value the model's enum holds, alone and beside
    // the members this account may write.
    let mut cases: Vec<(String, Value)> = Vec::new();
    for visibility in MEMBER_ACCOUNT_DISCOUNT_VISIBILITY_VALUES {
        cases.push((
            format!("{visibility} alone"),
            json!({"memberAccountDiscountVisibility": visibility}),
        ));
        cases.push((
            format!("{visibility} beside a preference the account may write"),
            json!({
                "memberAccountDiscountVisibility": visibility,
                "savingsEstimationMode": "BeforeDiscounts",
            }),
        ));
        cases.push((
            format!("{visibility} beside a nested member"),
            json!({
                "memberAccountDiscountVisibility": visibility,
                "preferredCommitment": {"term": "ThreeYears"},
            }),
        ));
    }

    for (case, request) in &cases {
        refused(
            &router,
            case,
            "UpdatePreferences",
            request,
            400,
            "ValidationException",
        )
        .await;

        // Nothing the refused request carried was stored: not the restricted
        // member, and not the members it was sent beside.
        let fetched = ok(&router, "GetPreferences", &json!({})).await;
        assert_eq!(
            fetched, before,
            "case {case:?}: a refused UpdatePreferences must leave the record at {before}, \
             got {fetched}"
        );
        assert!(
            fetched["memberAccountDiscountVisibility"].is_null(),
            "case {case:?}: expected memberAccountDiscountVisibility never reported, got {fetched}"
        );
    }
}

/// `savingsEstimationMode=AfterDiscounts` needs member-account discount
/// visibility enabled first — which only the management account can do — so it
/// is refused with 400 `ValidationException` and the stored mode is unchanged,
/// while `BeforeDiscounts` is accepted.
#[tokio::test]
async fn after_discounts_is_refused_until_discount_visibility_is_enabled() {
    let router = router();

    // (case, request, the status it must get, the mode a read must report
    // afterwards — the stored preference, or the service's default where the
    // account has set none). The accepted rows are the positive control: the
    // same member, the other value.
    let cases: [(&str, Value, u16, &str); 6] = [
        (
            // The account has set no mode yet, so a read reports the default
            // the service applies; the refused call does not make it the
            // account's own.
            "AfterDiscounts on a fresh record",
            json!({"savingsEstimationMode": "AfterDiscounts"}),
            400,
            "AfterDiscounts",
        ),
        (
            "BeforeDiscounts is accepted",
            json!({"savingsEstimationMode": "BeforeDiscounts"}),
            200,
            "BeforeDiscounts",
        ),
        (
            "AfterDiscounts over a stored BeforeDiscounts",
            json!({"savingsEstimationMode": "AfterDiscounts"}),
            400,
            "BeforeDiscounts",
        ),
        (
            "AfterDiscounts beside a nested member",
            json!({
                "savingsEstimationMode": "AfterDiscounts",
                "preferredCommitment": {"term": "OneYear"},
            }),
            400,
            "BeforeDiscounts",
        ),
        (
            // Asking for the mode and the visibility it depends on in one call
            // is still refused: enabling the visibility is the management
            // account's act, so this account cannot reach the mode either way.
            "AfterDiscounts beside the visibility it depends on",
            json!({
                "savingsEstimationMode": "AfterDiscounts",
                "memberAccountDiscountVisibility": "All",
            }),
            400,
            "BeforeDiscounts",
        ),
        (
            "BeforeDiscounts is still accepted afterwards",
            json!({"savingsEstimationMode": "BeforeDiscounts"}),
            200,
            "BeforeDiscounts",
        ),
    ];

    for (case, request, expected_status, expected_mode) in &cases {
        // What a read reports before the call, so a refusal can be shown to
        // have changed nothing at all.
        let before = ok(&router, "GetPreferences", &json!({})).await;
        if *expected_status == 200 {
            ok(&router, "UpdatePreferences", request).await;
        } else {
            refused(
                &router,
                case,
                "UpdatePreferences",
                request,
                *expected_status,
                "ValidationException",
            )
            .await;
        }

        let fetched = ok(&router, "GetPreferences", &json!({})).await;
        assert_eq!(
            fetched["savingsEstimationMode"].as_str(),
            Some(*expected_mode),
            "case {case:?}: expected the reported mode {expected_mode:?}, got {fetched}"
        );
        if *expected_status != 200 {
            assert_eq!(
                fetched, before,
                "case {case:?}: a refused UpdatePreferences must leave the record at {before}, \
                 got {fetched}"
            );
        }
        assert!(
            fetched["memberAccountDiscountVisibility"].is_null(),
            "case {case:?}: expected memberAccountDiscountVisibility never reported, got {fetched}"
        );
    }
}

/// A populated `filter.accountIds` asks `ListRecommendations` for other
/// accounts' recommendations, so it is refused with 403
/// `AccessDeniedException` — for any value, including the caller's own account
/// id — while the identical request without that member is served.
#[tokio::test]
async fn an_account_ids_filter_is_refused_to_a_non_management_account() {
    // An enrolled account whose analysis has come due, because the positive
    // controls below are about what the FILTER selects, not about enrollment.
    let router = enrolled_router().await;
    let account = engine::emulator_account_id();

    // (case, the filter sent). The rows differ in which ids the member names,
    // how many, and what else the filter narrows by.
    let cases: Vec<(String, Value)> = vec![
        (
            "the caller's own account".to_string(),
            json!({"accountIds": [account]}),
        ),
        (
            "an account that owns nothing in the catalog".to_string(),
            json!({"accountIds": ["111122223333"]}),
        ),
        (
            "two accounts".to_string(),
            json!({"accountIds": ["111122223333", "444455556666"]}),
        ),
        (
            "beside another filter member".to_string(),
            json!({"accountIds": [account], "regions": ["us-east-1"]}),
        ),
    ];

    for (case, filter) in &cases {
        refused(
            &router,
            case,
            "ListRecommendations",
            &json!({"includeAllRecommendations": true, "filter": filter, "maxResults": 10}),
            403,
            "AccessDeniedException",
        )
        .await;

        // The same request with the organization-scoped member dropped is
        // served: it is that member, not the rest of the filter, that the
        // account lacks the view for.
        let mut single_account = filter.clone();
        single_account
            .as_object_mut()
            .expect("the filter is an object")
            .remove("accountIds");
        let served = ok(
            &router,
            "ListRecommendations",
            &json!({
                "includeAllRecommendations": true,
                "filter": single_account,
                "maxResults": 10,
            }),
        )
        .await;
        assert!(
            served["items"].is_array(),
            "case {case:?}: expected the unfiltered-by-account call to list items, got {served}"
        );
    }

    // And with no filter at all.
    let all = ok(
        &router,
        "ListRecommendations",
        &json!({"includeAllRecommendations": true, "maxResults": 10}),
    )
    .await;
    assert!(
        all["items"]
            .as_array()
            .is_some_and(|items| !items.is_empty()),
        "case no filter: expected the catalog, got {all}"
    );
}

/// `ListEnrollmentStatuses` answers the caller's own record and refuses, with
/// 403 `AccessDeniedException`, a request for the organization's enrollment or
/// for another account's.
#[tokio::test]
async fn an_organization_enrollment_read_is_refused_to_a_non_management_account() {
    let router = router();
    let account = engine::emulator_account_id();

    // (case, request, the status it must get)
    let cases: Vec<(String, Value, u16)> = vec![
        (
            "the caller's own record".to_string(),
            json!({"includeOrganizationInfo": false}),
            200,
        ),
        (
            "the caller's own record, named".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": account}),
            200,
        ),
        (
            "the organization's enrollment".to_string(),
            json!({"includeOrganizationInfo": true}),
            403,
        ),
        (
            "another member account".to_string(),
            json!({"includeOrganizationInfo": false, "accountId": "111122223333"}),
            403,
        ),
        (
            "another member account, with the organization asked for too".to_string(),
            json!({"includeOrganizationInfo": true, "accountId": "111122223333"}),
            403,
        ),
        (
            "the organization's enrollment, paged".to_string(),
            json!({"includeOrganizationInfo": true, "maxResults": 10}),
            403,
        ),
    ];

    for (case, request, expected) in &cases {
        if *expected == 200 {
            let listed = ok(&router, "ListEnrollmentStatuses", request).await;
            let items = listed["items"].as_array().expect("items is a list");
            assert_eq!(
                items.len(),
                1,
                "case {case:?}: expected exactly the caller's record, got {listed}"
            );
            assert_eq!(
                items[0]["accountId"].as_str(),
                Some(account),
                "case {case:?}: expected the caller's account, got {listed}"
            );
        } else {
            refused(
                &router,
                case,
                "ListEnrollmentStatuses",
                request,
                403,
                "AccessDeniedException",
            )
            .await;
        }
    }
}

/// Cost Optimization Hub holds an enrollment record for the account it knows,
/// so the caller's status is readable BEFORE any `UpdateEnrollmentStatus`: one
/// item, `Inactive`, with both timestamps — and the record a read reports does
/// not change what a later reader sees.
#[tokio::test]
async fn the_callers_enrollment_is_readable_before_any_write() {
    let router = router();
    let account = engine::emulator_account_id();

    let first = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    let items = first["items"].as_array().expect("items is a list");
    assert_eq!(
        items.len(),
        1,
        "case fresh state: expected the caller's enrollment record, got {first}"
    );
    let record = &items[0];
    assert_eq!(
        record["accountId"].as_str(),
        Some(account),
        "case fresh state: expected account {account}, got {first}"
    );
    assert_eq!(
        record["status"].as_str(),
        Some("Inactive"),
        "case fresh state: expected an account that has not opted in, got {first}"
    );
    for member in ["createdTimestamp", "lastUpdatedTimestamp"] {
        assert!(
            record[member].as_f64().is_some(),
            "case fresh state: expected {member} on the record, got {first}"
        );
    }

    // Reading it twice answers the same record: the read materialises nothing.
    let second = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    assert_eq!(
        first, second,
        "case repeated read: the enrollment record changed between two reads"
    );

    // And the transition every client does next keeps the record's identity:
    // the same account, the asked-for status, the same createdTimestamp.
    let enrolled = ok(&router, "UpdateEnrollmentStatus", &json!({"status": "Active"})).await;
    assert_eq!(
        enrolled["status"].as_str(),
        Some("Active"),
        "case Active transition: expected the stored status, got {enrolled}"
    );
    let after = ok(
        &router,
        "ListEnrollmentStatuses",
        &json!({"includeOrganizationInfo": false}),
    )
    .await;
    let updated = &after["items"][0];
    assert_eq!(
        updated["status"].as_str(),
        Some("Active"),
        "case Active transition: expected the record to report Active, got {after}"
    );
    assert_eq!(
        updated["createdTimestamp"], record["createdTimestamp"],
        "case Active transition: createdTimestamp must stay at the record's creation, got {after}"
    );
}
