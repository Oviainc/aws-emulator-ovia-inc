// SPDX-License-Identifier: CC0-1.0

//! A dashboard's name identifies it within the account, end to end over the wire.
//!
//! The rule under test: a dashboard's name is unique within the account, so an operation
//! that ASSIGNS a name to a dashboard (`CreateDashboard`, and `UpdateDashboard` whose
//! `name` is `@required`) is refused with the modelled `ValidationException` (HTTP 400 —
//! neither operation declares a conflict error) and stores nothing when another stored
//! dashboard already carries that name; a name no stored dashboard carries — including one
//! a delete or a rename just freed, and one differing from a stored name only in case — is
//! accepted. Source for the rule: `CreateDashboardRequest.name` is documented "The name of
//! the dashboard. The name must be unique within your account."
//! (`smithy_query(query="doc",shape="CreateDashboardRequest",member="name")`).
//!
//! Every call goes through the real router (`POST /` with
//! `X-Amz-Target: AWSBCMDashboardsService.<Op>`, awsJson1_0), so what these tests assert is
//! what an SDK client sees, and every acceptance is read back through `GetDashboard` /
//! `ListDashboards` from the persisted record.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{call, error_code, graph_config, ok_call, router};

/// The names this suite spans. Rows differ in length (both published bounds of
/// `DashboardName`, 2..50), in which of the admitted character classes they use (letters,
/// digits, spaces, `_`, `-`) and in case. Source: the `DashboardName` member reported by
/// `smithy_query(query="operation",operation="CreateDashboard")` — length 2..50, pattern
/// `^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$`.
fn names() -> Vec<(&'static str, String)> {
    vec![
        ("at the 2-character minimum", "Em".to_string()),
        ("a plain two-word name", "Monthly Spend".to_string()),
        (
            "the same two words in lower case",
            "monthly spend".to_string(),
        ),
        ("every admitted character class", "A_b-C 1".to_string()),
        ("digits only after the first letter", "D2025".to_string()),
        (
            "at the 50-character maximum",
            format!("A{}z", "b".repeat(48)),
        ),
    ]
}

/// The widget shapes a create may carry, so the second (refused) create differs from the
/// first in every member but `name`.
fn one_widget() -> Value {
    json!([{"title": "Spend", "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]}])
}

/// Create a dashboard and return the ARN it was stored under.
async fn create(router: &MultiServiceRouter, request: &Value) -> String {
    let created = ok_call(router, "CreateDashboard", request).await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("CreateDashboard must answer an arn: {created}"))
        .to_string()
}

/// Every ARN `ListDashboards` reports, paired with the name it carries.
async fn listed(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let page = ok_call(router, "ListDashboards", &json!({"maxResults": 100})).await;
    page["dashboards"]
        .as_array()
        .unwrap_or_else(|| panic!("ListDashboards must answer a list: {page}"))
        .iter()
        .map(|item| {
            (
                item["arn"].as_str().unwrap_or("<none>").to_string(),
                item["name"].as_str().unwrap_or("<none>").to_string(),
            )
        })
        .collect()
}

/// A create naming a dashboard the account already holds is refused with
/// `ValidationException` (400) and stores nothing — whatever else the request carries.
#[tokio::test]
async fn a_create_under_a_stored_name_is_refused() {
    for (label, name) in names() {
        let router = router();
        let first = create(
            &router,
            &json!({"name": name, "widgets": [], "description": "The first one"}),
        )
        .await;

        // Same name, every other member different: the name alone decides.
        let (status, body) = call(
            &router,
            "CreateDashboard",
            &json!({
                "name": name,
                "widgets": one_widget(),
                "resourceTags": [{"key": "team", "value": "finops"}]
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label}: a create under the stored name {name:?} must be refused, got {status}: {body}"
        );
        assert!(
            error_code(&body).ends_with("ValidationException"),
            "case {label}: expected ValidationException, got {}: {body}",
            error_code(&body)
        );
        let message = body["message"].as_str().unwrap_or("<no message>");
        assert!(
            message.contains(&name) && message.contains("name"),
            "case {label}: the refusal must name the member and the value it refused, got {message:?}"
        );

        // Nothing was stored: one dashboard, the first one, unchanged.
        let stored = listed(&router).await;
        assert_eq!(
            stored,
            vec![(first.clone(), name.clone())],
            "case {label}: the refused create must have stored nothing, got {stored:?}"
        );
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": first})).await;
        assert_eq!(
            fetched["description"].as_str(),
            Some("The first one"),
            "case {label}: the refused create must not have touched the stored record: {fetched}"
        );
        assert_eq!(
            fetched["widgets"].as_array().map(Vec::len),
            Some(0),
            "case {label}: the refused create must not have touched the stored widgets: {fetched}"
        );
    }
}

/// Names that differ — including two that differ only in case — are distinct dashboards:
/// every row of the table is accepted on ONE account and reads back under its own ARN.
#[tokio::test]
async fn names_that_differ_are_each_accepted_once() {
    let router = router();
    let mut minted: Vec<(String, String)> = Vec::new();

    for (label, name) in names() {
        let arn = create(&router, &json!({"name": name, "widgets": one_widget()})).await;
        assert!(
            !minted.iter().any(|(stored, _)| stored == &arn),
            "case {label}: every create mints a distinct arn, but {arn:?} repeats {minted:?}"
        );
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        assert_eq!(
            fetched["name"].as_str(),
            Some(name.as_str()),
            "case {label}: GetDashboard must report the stored name: {fetched}"
        );
        minted.push((arn, name));
    }

    let mut stored = listed(&router).await;
    stored.sort();
    minted.sort();
    assert_eq!(
        stored, minted,
        "every distinct name must be stored exactly once, got {stored:?}"
    );
}

/// A name is held by a dashboard, not by the account for ever: deleting the dashboard
/// frees it, and the next create takes it under a FRESH arn.
#[tokio::test]
async fn a_name_a_delete_freed_can_be_taken_again() {
    for (label, name) in names() {
        let router = router();
        let first = create(&router, &json!({"name": name, "widgets": []})).await;
        ok_call(&router, "DeleteDashboard", &json!({"arn": first})).await;

        let second = create(&router, &json!({"name": name, "widgets": one_widget()})).await;
        assert_ne!(
            second, first,
            "case {label}: a re-created dashboard must not reuse the deleted dashboard's arn"
        );
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": second})).await;
        assert_eq!(
            fetched["name"].as_str(),
            Some(name.as_str()),
            "case {label}: the re-created dashboard must carry the freed name: {fetched}"
        );
        let stored = listed(&router).await;
        assert_eq!(
            stored,
            vec![(second, name.clone())],
            "case {label}: the account must hold exactly the re-created dashboard, got {stored:?}"
        );
    }
}

/// `UpdateDashboard` assigns a name too, so it obeys the same rule: a name another stored
/// dashboard carries is refused and changes nothing, the dashboard's own current name is
/// accepted (it collides with nothing but itself), and a free name is taken.
#[tokio::test]
async fn update_dashboard_holds_the_name_to_the_same_rule() {
    // (label, the name the SECOND dashboard is renamed to, accepted)
    let cases: Vec<(&str, String, bool)> = vec![
        ("the other dashboard's name", "Alpha Costs".to_string(), false),
        (
            "the other dashboard's name in lower case",
            "alpha costs".to_string(),
            true,
        ),
        ("its own current name", "Bravo Costs".to_string(), true),
        ("a name nobody holds", "Charlie Costs".to_string(), true),
        ("a free name at the 2-character minimum", "Em".to_string(), true),
        (
            "a free name at the 50-character maximum",
            format!("A{}z", "b".repeat(48)),
            true,
        ),
    ];

    for (label, new_name, accepted) in cases {
        let router = router();
        let alpha = create(&router, &json!({"name": "Alpha Costs", "widgets": []})).await;
        let bravo = create(
            &router,
            &json!({"name": "Bravo Costs", "widgets": one_widget(), "description": "Before"}),
        )
        .await;

        let (status, body) = call(
            &router,
            "UpdateDashboard",
            &json!({"arn": bravo, "name": new_name, "description": "After"}),
        )
        .await;

        if accepted {
            assert_eq!(
                status, 200,
                "case {label}: {new_name:?} is held by no other dashboard, got {status}: {body}"
            );
            let fetched = ok_call(&router, "GetDashboard", &json!({"arn": bravo})).await;
            assert_eq!(
                fetched["name"].as_str(),
                Some(new_name.as_str()),
                "case {label}: the new name must be stored: {fetched}"
            );
            assert_eq!(
                fetched["description"].as_str(),
                Some("After"),
                "case {label}: the accepted update must have been applied: {fetched}"
            );
            // The name the rename freed is now available to the other dashboard.
            if new_name != "Bravo Costs" {
                ok_call(
                    &router,
                    "UpdateDashboard",
                    &json!({"arn": alpha, "name": "Bravo Costs"}),
                )
                .await;
                let renamed = ok_call(&router, "GetDashboard", &json!({"arn": alpha})).await;
                assert_eq!(
                    renamed["name"].as_str(),
                    Some("Bravo Costs"),
                    "case {label}: a name a rename freed must be takeable: {renamed}"
                );
            }
        } else {
            assert_eq!(
                status, 400,
                "case {label}: {new_name:?} is held by another dashboard, got {status}: {body}"
            );
            assert!(
                error_code(&body).ends_with("ValidationException"),
                "case {label}: expected ValidationException, got {}: {body}",
                error_code(&body)
            );
            // The refused update changed nothing on either dashboard.
            let fetched = ok_call(&router, "GetDashboard", &json!({"arn": bravo})).await;
            assert_eq!(
                fetched["name"].as_str(),
                Some("Bravo Costs"),
                "case {label}: the refused update must leave the name alone: {fetched}"
            );
            assert_eq!(
                fetched["description"].as_str(),
                Some("Before"),
                "case {label}: the refused update must leave the record alone: {fetched}"
            );
            let other = ok_call(&router, "GetDashboard", &json!({"arn": alpha})).await;
            assert_eq!(
                other["name"].as_str(),
                Some("Alpha Costs"),
                "case {label}: the refused update must leave the name holder alone: {other}"
            );
        }
    }
}
