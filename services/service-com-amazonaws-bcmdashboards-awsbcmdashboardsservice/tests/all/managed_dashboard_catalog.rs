// SPDX-License-Identifier: CC0-1.0

//! The AWS-managed dashboard catalog every account already holds, end to end over the
//! wire.
//!
//! The rule under test: this service owns a fixed set of dashboards of its own, so an
//! account that has created nothing still enumerates them — `ListDashboards` reports one
//! `DashboardReference` per catalog entry, carrying `type: AWS_MANAGED`, and
//! `GetDashboard` reads each of them back with its widgets — and they live in the one
//! dashboard store, so they page, project and read exactly as a dashboard a caller
//! created does, alongside the account's own `CUSTOM` dashboards.
//!
//! The expectations here are written out rather than read back from the service: the five
//! entries' NAMES come from the wire record of a brand-new account
//! (`bcm-dashboards-catalog-list-dashboards`), and every other expectation is the model's
//! own — `DashboardReference` requires `arn`, `name`, `type`, `createdAt` and `updatedAt`
//! (`smithy_query(query="shape",shape="DashboardReference")`); `GetDashboard` additionally
//! requires `widgets`; `DashboardArn`'s `@pattern` fixes the ARN's shape; `WidgetId` is 32
//! hexadecimal characters, `WidgetTitle` 2..50 characters and `WidgetWidth`/`WidgetHeight`
//! range 2..6 / 4..10 (`smithy_query(query="shape",shape="Widget")`).
//!
//! Every call goes through the real router, so what these tests assert is what an SDK
//! client sees.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::dashboard_lifecycle::{graph_config, ok_call, router};

/// The `DashboardType` the catalog's entries carry on the wire.
const AWS_MANAGED: &str = "AWS_MANAGED";

/// The `DashboardType` a dashboard a caller creates carries — the only value the model's
/// enum declares.
const CUSTOM: &str = "CUSTOM";

/// The names of the five AWS-managed dashboards a brand-new account enumerates, as the
/// wire record of `ListDashboards` on one shows them.
const CATALOG_NAMES: [&str; 5] = [
    "Cost Overview and Trends",
    "Compute",
    "Database",
    "Reservations",
    "Savings Plans",
];

/// Whether `id` is 32 lowercase hexadecimal characters — `WidgetId`'s modelled
/// `@pattern` `^[0-9a-f]{32}$` and its 32..32 `@length`.
fn is_widget_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c))
}

/// Whether `arn` is a managed dashboard's ARN: `DashboardArn`'s modelled shape
/// (`^arn:aws[-a-z0-9]*:bcm-dashboards::[0-9]{12}:dashboard/(\*|[-a-z0-9]+)$`, no Region
/// segment) with the `managed-` resource-id prefix the catalog's ids carry.
fn is_managed_dashboard_arn(arn: &str) -> bool {
    let Some(rest) = arn.strip_prefix("arn:aws:bcm-dashboards::") else {
        return false;
    };
    let Some((account, resource)) = rest.split_once(':') else {
        return false;
    };
    let Some(resource_id) = resource.strip_prefix("dashboard/managed-") else {
        return false;
    };
    account.len() == 12
        && account.chars().all(|c| c.is_ascii_digit())
        && is_widget_id(resource_id)
}

/// Every dashboard reference `ListDashboards` reports, walking every page.
async fn every_reference(router: &MultiServiceRouter, page_size: usize) -> Vec<Value> {
    let mut references: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut request = json!({"maxResults": page_size});
        if let Some(token) = &token {
            request["nextToken"] = json!(token);
        }
        let page = ok_call(router, "ListDashboards", &request).await;
        let items = page["dashboards"]
            .as_array()
            .unwrap_or_else(|| panic!("case page size {page_size}: no dashboards: {page}"))
            .clone();
        assert!(
            items.len() <= page_size,
            "case page size {page_size}: a page carried {} references",
            items.len()
        );
        references.extend(items);
        token = page
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return references;
        }
        assert!(
            references.len() <= 1_000,
            "case page size {page_size}: pagination did not terminate"
        );
    }
}

/// The references of `type: AWS_MANAGED`, in the order the listing reported them.
fn managed_references(references: &[Value]) -> Vec<Value> {
    references
        .iter()
        .filter(|item| item["type"].as_str() == Some(AWS_MANAGED))
        .cloned()
        .collect()
}

/// A brand-new account enumerates the whole catalog before it has written anything: one
/// reference per entry, every modelled member present, each carrying `AWS_MANAGED` and an
/// ARN of the managed form — and nothing else is listed.
#[tokio::test]
async fn a_fresh_account_lists_the_whole_managed_catalog() {
    let router = router();

    let references = every_reference(&router, 100).await;
    let names: Vec<&str> = references
        .iter()
        .map(|item| item["name"].as_str().unwrap_or("<none>"))
        .collect();
    let expected: BTreeSet<&str> = CATALOG_NAMES.into_iter().collect();
    let listed: BTreeSet<&str> = names.iter().copied().collect();
    assert_eq!(
        listed, expected,
        "case a fresh account: expected exactly the managed catalog {expected:?}, got {names:?}"
    );

    let mut seen_arns: BTreeSet<&str> = BTreeSet::new();
    for item in &references {
        let name = item["name"].as_str().unwrap_or("<none>");
        let arn = item["arn"].as_str().unwrap_or("<none>");
        assert!(
            is_managed_dashboard_arn(arn),
            "case {name:?}: expected an arn of the form \
             arn:aws:bcm-dashboards::<account>:dashboard/managed-<32 hex>, got {arn:?}"
        );
        assert!(
            seen_arns.insert(arn),
            "case {name:?}: every catalog entry has its own arn, but {arn:?} repeats"
        );
        assert_eq!(
            item["type"].as_str(),
            Some(AWS_MANAGED),
            "case {name:?}: a catalog entry is AWS-managed, got {item}"
        );
        let description = item["description"].as_str().unwrap_or("");
        assert!(
            (1..=200).contains(&description.chars().count()),
            "case {name:?}: expected a description inside the modelled 1..200 length, got \
             {description:?}"
        );
        for member in ["createdAt", "updatedAt"] {
            assert!(
                item[member].as_f64().is_some_and(|stamp| stamp > 0.0),
                "case {name:?}: {member} is @required on a reference, got {item}"
            );
        }
    }
}

/// Every catalog entry reads back through `GetDashboard`: the reference's members, plus
/// the widgets the model requires on the detail read — each widget inside its modelled
/// bounds and carrying a query and a display configuration.
#[tokio::test]
async fn every_catalog_entry_reads_back_with_its_widgets() {
    let router = router();

    for reference in every_reference(&router, 100).await {
        let arn = reference["arn"].as_str().expect("an arn").to_string();
        let name = reference["name"].as_str().unwrap_or("<none>").to_string();
        let fetched = ok_call(&router, "GetDashboard", &json!({"arn": arn})).await;

        for member in ["arn", "name", "description", "type", "createdAt", "updatedAt"] {
            assert_eq!(
                fetched.get(member),
                reference.get(member),
                "case {name:?}: the reference and the detail read are one record, but {member} \
                 differs: {reference} vs {fetched}"
            );
        }

        let widgets = fetched["widgets"]
            .as_array()
            .unwrap_or_else(|| panic!("case {name:?}: widgets is @required: {fetched}"))
            .clone();
        assert!(
            !widgets.is_empty(),
            "case {name:?}: a managed dashboard is published with its widgets, got {fetched}"
        );
        for widget in &widgets {
            let id = widget["id"].as_str().unwrap_or("<none>");
            assert!(
                is_widget_id(id),
                "case {name:?}: every widget carries a 32-hex WidgetId, got {id:?}"
            );
            let title = widget["title"].as_str().unwrap_or("");
            assert!(
                (2..=50).contains(&title.chars().count()),
                "case {name:?}: expected a title inside the modelled 2..50 length, got {title:?}"
            );
            let width = widget["width"].as_i64().unwrap_or_default();
            assert!(
                (2..=6).contains(&width),
                "case {name:?}: expected a width inside the modelled 2..6 range, got {width}"
            );
            let height = widget["height"].as_i64().unwrap_or_default();
            assert!(
                (4..=10).contains(&height),
                "case {name:?}: expected a height inside the modelled 4..10 range, got {height}"
            );
            let configs = widget["configs"]
                .as_array()
                .unwrap_or_else(|| panic!("case {name:?}: configs is @required: {widget}"));
            assert!(
                (1..=2).contains(&configs.len()),
                "case {name:?}: expected 1..2 configs, got {}",
                configs.len()
            );
            for config in configs {
                assert!(
                    config["queryParameters"].is_object(),
                    "case {name:?}: queryParameters is @required on a config, got {config}"
                );
                assert!(
                    config["displayConfig"].is_object(),
                    "case {name:?}: displayConfig is @required on a config, got {config}"
                );
            }
        }
    }
}

/// The catalog is in the one dashboard store: it pages by the same cursor rule as the
/// account's own dashboards, at every page size, and a caller's dashboards are listed
/// beside it — distinguishable by `type` alone.
#[tokio::test]
async fn the_catalog_pages_and_lists_beside_the_accounts_own_dashboards() {
    let router = router();

    // Page sizes from one to past the catalog's size: the walk must report each entry
    // exactly once however the pages fall.
    let whole = every_reference(&router, 100).await;
    let whole_arns: Vec<&str> = whole
        .iter()
        .map(|item| item["arn"].as_str().unwrap_or("<none>"))
        .collect();
    for page_size in [1_usize, 2, 3, 5, 100] {
        let paged = every_reference(&router, page_size).await;
        let paged_arns: Vec<&str> = paged
            .iter()
            .map(|item| item["arn"].as_str().unwrap_or("<none>"))
            .collect();
        assert_eq!(
            paged_arns, whole_arns,
            "case page size {page_size}: paging must walk the catalog exactly once, in order"
        );
    }

    // A dashboard of the account's own, created beside the catalog.
    let created = ok_call(
        &router,
        "CreateDashboard",
        &json!({
            "name": "My Own Dashboard",
            "widgets": [{"title": "Spend", "configs": [graph_config("UnblendedCost", "LINE", "MONTHLY")]}]
        }),
    )
    .await;
    let own_arn = created["arn"].as_str().expect("an arn").to_string();

    let after = every_reference(&router, 2).await;
    assert_eq!(
        after.len(),
        whole.len() + 1,
        "case a create beside the catalog: expected the catalog plus one dashboard, got {}",
        after.len()
    );
    let own: Vec<&Value> = after
        .iter()
        .filter(|item| item["arn"].as_str() == Some(own_arn.as_str()))
        .collect();
    assert_eq!(
        own.len(),
        1,
        "case a create beside the catalog: the created dashboard must be listed once, got {own:?}"
    );
    assert_eq!(
        own[0]["type"].as_str(),
        Some(CUSTOM),
        "case a create beside the catalog: a caller's dashboard is CUSTOM, got {}",
        own[0]
    );
    assert_eq!(
        managed_references(&after).len(),
        whole.len(),
        "case a create beside the catalog: the catalog must be untouched by the create: {after:?}"
    );
}

/// Reading the catalog changes nothing a later caller sees: the same listing and the same
/// detail reads answer the same records, and a read never adds a dashboard.
#[tokio::test]
async fn reading_the_catalog_changes_nothing() {
    let router = router();

    let before = every_reference(&router, 100).await;
    let mut details: Vec<Value> = Vec::new();
    for reference in &before {
        let arn = reference["arn"].as_str().expect("an arn").to_string();
        details.push(ok_call(&router, "GetDashboard", &json!({"arn": arn})).await);
        // A policy read of a managed dashboard is a read too: it must neither fail nor
        // write, and with no policy attached it reports the empty JSON document.
        let policy = ok_call(
            &router,
            "GetResourcePolicy",
            &json!({"resourceArn": reference["arn"].as_str().expect("an arn")}),
        )
        .await;
        assert_eq!(
            policy["policyDocument"].as_str(),
            Some("{}"),
            "case {}: a dashboard with no policy reports the empty JSON document, got {policy}",
            reference["name"]
        );
    }

    let after = every_reference(&router, 100).await;
    assert_eq!(
        after, before,
        "reading the catalog must leave the listing identical, got {after:?} after {before:?}"
    );
    for (reference, detail) in before.iter().zip(details.iter()) {
        let again = ok_call(
            &router,
            "GetDashboard",
            &json!({"arn": reference["arn"].as_str().expect("an arn")}),
        )
        .await;
        assert_eq!(
            &again, detail,
            "case {}: a second detail read must answer the same record, got {again} after {detail}",
            reference["name"]
        );
    }
}
