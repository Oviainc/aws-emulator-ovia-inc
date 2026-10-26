// SPDX-License-Identifier: CC0-1.0

//! `UpdateDashboard` is a PARTIAL update: an optional member the request omits keeps the
//! value the record already holds.
//!
//! The rule under test: an update operation replaces an optional member only when the
//! request carries it, and leaves the stored value untouched when the request omits it —
//! clearing is asked for explicitly, never inferred from an absent member. This service
//! says so in its own model: `UpdateScheduledReportRequest` carries `clearWidgetIds` and
//! `clearWidgetDateRangeOverride` (boolean, `@default false`) for exactly that, and
//! `UpdateDashboardRequest` models no such flag for `description` or `widgets` (source:
//! `smithy_query(query="shape",shape="UpdateDashboardRequest,UpdateScheduledReportRequest")`).
//!
//! The rows below span both optional members of the request (`description`, `widgets`),
//! present and absent, over records that hold a value and records that hold none, and
//! over `Description`'s published length bounds (len 1..200, same source) — so what the
//! table asserts is the rule, not the one shape of request that exposed it.
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees: the members a read reports come from the persisted record.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{graph_config, ok_call, router};

/// `Description`'s modelled minimum length (1) as a value: the shortest description a
/// record can hold.
fn shortest_description() -> String {
    "A".to_string()
}

/// `Description`'s modelled maximum length (200) as a value, built from characters its
/// `@pattern` admits and with no double space.
fn longest_description() -> String {
    "D".repeat(200)
}

/// A widget the create and update requests carry, titled `title`.
fn widget(title: &str) -> Value {
    json!({"title": title, "configs": [graph_config("UnblendedCost", "BAR", "DAILY")]})
}

/// One row of the table: what the create persists, what the update carries beyond the
/// two @required members, and what a later read must report.
struct Case {
    label: &'static str,
    /// The description the create request carries, `None` for a create that carries none.
    stored_description: Option<String>,
    /// The widget titles the create request carries.
    stored_titles: Vec<&'static str>,
    /// The OPTIONAL members the update request carries alongside `arn` and `name`.
    update: Value,
    /// The description a read must report after the update.
    expected_description: Option<String>,
    /// The widget titles a read must report after the update.
    expected_titles: Vec<&'static str>,
}

fn cases() -> Vec<Case> {
    vec![
        Case {
            label: "both optional members absent: a plain rename keeps description and widgets",
            stored_description: Some("The stored description".to_string()),
            stored_titles: vec!["Spend", "Coverage"],
            update: json!({}),
            expected_description: Some("The stored description".to_string()),
            expected_titles: vec!["Spend", "Coverage"],
        },
        Case {
            label: "description present: it replaces the stored one",
            stored_description: Some("The stored description".to_string()),
            stored_titles: vec!["Spend"],
            update: json!({"description": "A replacement description"}),
            expected_description: Some("A replacement description".to_string()),
            expected_titles: vec!["Spend"],
        },
        Case {
            label: "widgets present, description absent: widgets replaced, description kept",
            stored_description: Some("The stored description".to_string()),
            stored_titles: vec!["Spend", "Coverage"],
            update: json!({"widgets": [{"title": "Only-One", "configs": [
                graph_config("AmortizedCost", "STACK", "MONTHLY")
            ]}]}),
            expected_description: Some("The stored description".to_string()),
            expected_titles: vec!["Only-One"],
        },
        Case {
            label: "widgets present and empty, description absent: no widget left, description kept",
            stored_description: Some("The stored description".to_string()),
            stored_titles: vec!["Spend"],
            update: json!({"widgets": []}),
            expected_description: Some("The stored description".to_string()),
            expected_titles: vec![],
        },
        Case {
            label: "both members present: both replaced",
            stored_description: Some("The stored description".to_string()),
            stored_titles: vec!["Spend"],
            update: json!({"description": "Both at once", "widgets": [
                {"title": "First", "configs": [graph_config("UnblendedCost", "BAR", "DAILY")]},
                {"title": "Second", "configs": [graph_config("NetUnblendedCost", "LINE", "HOURLY")]}
            ]}),
            expected_description: Some("Both at once".to_string()),
            expected_titles: vec!["First", "Second"],
        },
        Case {
            label: "nothing stored to keep: an absent description stays absent",
            stored_description: None,
            stored_titles: vec!["Spend"],
            update: json!({}),
            expected_description: None,
            expected_titles: vec!["Spend"],
        },
        Case {
            label: "nothing stored to keep: a present description is set",
            stored_description: None,
            stored_titles: vec!["Spend"],
            update: json!({"description": "Described at last"}),
            expected_description: Some("Described at last".to_string()),
            expected_titles: vec!["Spend"],
        },
        Case {
            label: "a description at the 1-character minimum survives a rename",
            stored_description: Some(shortest_description()),
            stored_titles: vec!["Spend"],
            update: json!({}),
            expected_description: Some(shortest_description()),
            expected_titles: vec!["Spend"],
        },
        Case {
            label: "a description at the 200-character maximum survives a rename",
            stored_description: Some(longest_description()),
            stored_titles: vec![],
            update: json!({}),
            expected_description: Some(longest_description()),
            expected_titles: vec![],
        },
        Case {
            label: "a description at the 200-character maximum is replaceable by the shortest",
            stored_description: Some(longest_description()),
            stored_titles: vec!["Spend"],
            update: json!({"description": shortest_description()}),
            expected_description: Some(shortest_description()),
            expected_titles: vec!["Spend"],
        },
    ]
}

/// Create a dashboard for one row and return its ARN.
async fn create(router: &MultiServiceRouter, case: &Case) -> String {
    let mut request = json!({
        "name": "Before Update",
        "widgets": case.stored_titles.iter().map(|t| widget(t)).collect::<Vec<Value>>()
    });
    if let Some(description) = &case.stored_description {
        request["description"] = json!(description);
    }
    let created = ok_call(router, "CreateDashboard", &request).await;
    created["arn"]
        .as_str()
        .unwrap_or_else(|| panic!("case {}: CreateDashboard answered no arn: {created}", case.label))
        .to_string()
}

/// The widget titles a `GetDashboard` body reports, in order.
fn titles(body: &Value, label: &str) -> Vec<String> {
    body["widgets"]
        .as_array()
        .unwrap_or_else(|| panic!("case {label}: GetDashboard reported no widget list: {body}"))
        .iter()
        .map(|w| w["title"].as_str().unwrap_or("<none>").to_string())
        .collect()
}

/// The table: each optional member is replaced when the update carries it and kept as
/// stored when the update omits it.
#[tokio::test]
async fn an_omitted_optional_member_keeps_the_stored_value() {
    for case in cases() {
        let router = router();
        let arn = create(&router, &case).await;

        let mut request = json!({"arn": arn, "name": "After Update"});
        if let Value::Object(members) = &case.update {
            for (member, value) in members {
                request[member] = value.clone();
            }
        }
        ok_call(&router, "UpdateDashboard", &request).await;

        let after = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let label = case.label;
        let actual_description = after.get("description").and_then(Value::as_str);
        assert_eq!(
            actual_description,
            case.expected_description.as_deref(),
            "case {label:?}: expected description {:?}, got {actual_description:?}",
            case.expected_description
        );
        let actual_titles = titles(&after, label);
        assert_eq!(
            actual_titles, case.expected_titles,
            "case {label:?}: expected widgets {:?}, got {actual_titles:?}",
            case.expected_titles
        );
        assert_eq!(
            after["name"].as_str(),
            Some("After Update"),
            "case {label:?}: the @required name is always replaced, got {after}"
        );
    }
}

/// The invariant behind the table: an update that carries only the @required members is
/// value-preserving on every optional member, so running it any number of times reports
/// the same record as running it once. A clear inferred from an absent member would lose
/// the description on the FIRST of these calls.
#[tokio::test]
async fn repeated_rename_only_updates_never_erase_the_record() {
    let router = router();
    let description = "Survives every rename";
    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "Rename Me 0",
            "description": description,
            "widgets": [widget("Spend"), widget("Coverage")]
        }),
    )
    .await;
    let arn = created["arn"].as_str().expect("an arn").to_string();

    for round in 1..=4 {
        let name = format!("Rename Me {round}");
        ok_call(
            &router,
            "UpdateDashboard",
            &json!({"arn": arn, "name": name}),
        )
        .await;

        let after = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;
        let label = format!("rename round {round}");
        let actual_description = after.get("description").and_then(Value::as_str);
        assert_eq!(
            actual_description,
            Some(description),
            "case {label:?}: expected description {description:?}, got {actual_description:?}"
        );
        let actual_titles = titles(&after, &label);
        assert_eq!(
            actual_titles,
            vec!["Spend".to_string(), "Coverage".to_string()],
            "case {label:?}: expected the stored widgets, got {actual_titles:?}"
        );
        assert_eq!(
            after["name"].as_str(),
            Some(name.as_str()),
            "case {label:?}: expected name {name:?}, got {after}"
        );
    }

    // The listing projects the same preserved record, so this is the stored state and not
    // a per-read reconstruction.
    let listed = ok_call(&router, "ListDashboards", &json!({})).await;
    let listed_description = listed["dashboards"][0]
        .get("description")
        .and_then(Value::as_str);
    assert_eq!(
        listed_description,
        Some(description),
        "case \"listing after renames\": expected description {description:?}, got \
         {listed_description:?}"
    );
}
