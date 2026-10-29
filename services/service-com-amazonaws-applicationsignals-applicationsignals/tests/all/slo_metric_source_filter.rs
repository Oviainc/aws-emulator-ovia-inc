// SPDX-License-Identifier: CC0-1.0

//! A structure-valued `ListServiceLevelObjectives` selector narrows by EVERY member it states.
//!
//! The rule under test: a selector that is a whole structure is matched against the stored
//! record member for member — each member the request states is a further condition the stored
//! one meets, compared by the rule of its own type (an `Attributes` map by containment of the
//! entries named, an operation name by equality) — and only a member the request LEAVES OUT
//! narrows nothing. The rows below drive it over the two structure-valued selectors this read
//! takes (`MetricSource`, with both of its members, and the sibling `DependencyConfig`), and
//! over what the rule generalises over: which members the selector states, which entries each
//! stated map carries, how many of them (up to the modelled `Attributes` maximum of four), and
//! whether the stored record carries the member at all.
//!
//! `MetricSource` is `{ MetricSourceKeyAttributes (required), MetricSourceAttributes }` and
//! `Attributes` is a `length 1..4` map whose keys match `^[a-zA-Z]{1,50}$`
//! (`smithy_query(query="shape",shape="MetricSource,Attributes")`), so the published length
//! bounds of the additional attributes are exercised too: a four-entry map is accepted and an
//! empty or five-entry one is refused.
//!
//! The harness is `slo_lifecycle`'s, so this suite speaks to the same mounted router as every
//! other.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::slo_lifecycle::{call, create, router};

/// The `Type` key attribute of a RUM app monitor: a metric source outside Application Signals.
const APP_MONITOR_TYPE: &str = "AWS::RUM::AppMonitor";

/// The `Type` key attribute of a synthetics canary, the other metric source kind seeded here.
const CANARY_TYPE: &str = "AWS::Synthetics::Canary";

/// One `MetricSource`: the key attributes that identify it, plus the additional attributes
/// beside them when the caller states any.
fn metric_source(kind: &str, name: &str, additional: Option<Value>) -> Value {
    let mut source = json!({ "MetricSourceKeyAttributes": { "Type": kind, "Name": name } });
    if let Some(additional) = additional {
        source["MetricSourceAttributes"] = additional;
    }
    source
}

/// Every objective this suite stores, as (name, SLI metric config). The names sort in the
/// order the listing reports them, so each case below states its expectation in name order.
fn seeds() -> Vec<(&'static str, Value)> {
    vec![
        (
            // The same additional attributes as `ms-rum-checkout`, on another source.
            "ms-canary-checkout",
            json!({ "MetricSource": metric_source(CANARY_TYPE, "ping", Some(json!({ "Page": "checkout" }))) }),
        ),
        (
            "ms-dep-charge",
            json!({
                "KeyAttributes": { "Type": "Service", "Name": "orders", "Environment": "prod" },
                "DependencyConfig": {
                    "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "ledger" },
                    "DependencyOperationName": "Charge"
                }
            }),
        ),
        (
            "ms-dep-refund",
            json!({
                "KeyAttributes": { "Type": "Service", "Name": "orders", "Environment": "prod" },
                "DependencyConfig": {
                    "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "ledger" },
                    "DependencyOperationName": "Refund"
                }
            }),
        ),
        (
            // A source stored with NO additional attributes at all.
            "ms-rum-bare",
            json!({ "MetricSource": metric_source(APP_MONITOR_TYPE, "web", None) }),
        ),
        (
            "ms-rum-cart",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Page": "cart", "Team": "storefront" })),
                )
            }),
        ),
        (
            "ms-rum-checkout",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Page": "checkout", "Team": "storefront" })),
                )
            }),
        ),
        (
            // The modelled `Attributes` maximum of four additional attributes.
            "ms-rum-full",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({
                        "Page": "search",
                        "Team": "storefront",
                        "Tier": "gold",
                        "Region": "eu"
                    })),
                )
            }),
        ),
    ]
}

/// Every seeded objective's name, in the order the listing reports them.
fn every_seed() -> Vec<&'static str> {
    seeds().into_iter().map(|(name, _)| name).collect()
}

/// Store every entry of [`seeds`], so each case below has a set to select from.
async fn seed(router: &MultiServiceRouter) {
    for (name, metric) in seeds() {
        let (status, created) = create(
            router,
            json!({
                "Name": name,
                "SliConfig": {
                    "SliMetricConfig": metric,
                    "MetricThreshold": 100.0,
                    "ComparisonOperator": "LessThan"
                }
            }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: expected the setup create to answer 200, got {status} with {created}"
        );
    }
}

/// The objective names one listing reports, in order.
fn names(page: &Value) -> Vec<String> {
    page["SloSummaries"]
        .as_array()
        .map(|summaries| {
            summaries
                .iter()
                .map(|summary| summary["Name"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Every member a structure-valued selector states narrows the reported set, and a member it
/// leaves out narrows nothing.
#[tokio::test]
async fn a_structure_valued_selector_narrows_by_every_member_it_states() {
    let router = router();
    seed(&router).await;

    let cases: Vec<(&str, Value, Vec<&str>)> = vec![
        (
            "no selector at all",
            json!({}),
            every_seed(),
        ),
        (
            "MetricSource states the key attributes only, so the additional ones narrow nothing",
            json!({ "MetricSource": metric_source(APP_MONITOR_TYPE, "web", None) }),
            vec!["ms-rum-bare", "ms-rum-cart", "ms-rum-checkout", "ms-rum-full"],
        ),
        (
            "MetricSource states one additional attribute",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Page": "checkout" })),
                )
            }),
            vec!["ms-rum-checkout"],
        ),
        (
            "MetricSource states another value of that attribute",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Page": "cart" })),
                )
            }),
            vec!["ms-rum-cart"],
        ),
        (
            "MetricSource states an additional attribute two stored sources share",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Team": "storefront" })),
                )
            }),
            vec!["ms-rum-cart", "ms-rum-checkout", "ms-rum-full"],
        ),
        (
            "MetricSource states both of a stored source's additional attributes",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Page": "checkout", "Team": "storefront" })),
                )
            }),
            vec!["ms-rum-checkout"],
        ),
        (
            "MetricSource states two additional attributes, one of them with another value",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Page": "checkout", "Team": "payments" })),
                )
            }),
            vec![],
        ),
        (
            "MetricSource states an additional attribute no stored source carries",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Absent": "nothing" })),
                )
            }),
            vec![],
        ),
        (
            "MetricSource states the modelled maximum of four additional attributes",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({
                        "Page": "search",
                        "Team": "storefront",
                        "Tier": "gold",
                        "Region": "eu"
                    })),
                )
            }),
            vec!["ms-rum-full"],
        ),
        (
            "MetricSource states one of a four-attribute source's additional attributes",
            json!({
                "MetricSource": metric_source(
                    APP_MONITOR_TYPE,
                    "web",
                    Some(json!({ "Tier": "gold" })),
                )
            }),
            vec!["ms-rum-full"],
        ),
        (
            "MetricSource states those additional attributes against another source's key attributes",
            json!({
                "MetricSource": metric_source(
                    CANARY_TYPE,
                    "ping",
                    Some(json!({ "Page": "checkout" })),
                )
            }),
            vec!["ms-canary-checkout"],
        ),
        (
            "MetricSource names part of the key attributes beside an additional attribute",
            json!({
                "MetricSource": {
                    "MetricSourceKeyAttributes": { "Type": APP_MONITOR_TYPE },
                    "MetricSourceAttributes": { "Page": "checkout" }
                }
            }),
            vec!["ms-rum-checkout"],
        ),
        (
            // The sibling structure-valued selector: its two members are both required, and
            // each one narrows the same way.
            "DependencyConfig states the operation called on the dependency",
            json!({ "DependencyConfig": {
                "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "ledger" },
                "DependencyOperationName": "Charge"
            } }),
            vec!["ms-dep-charge"],
        ),
        (
            "DependencyConfig states another operation on the same dependency",
            json!({ "DependencyConfig": {
                "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "ledger" },
                "DependencyOperationName": "Refund"
            } }),
            vec!["ms-dep-refund"],
        ),
        (
            "DependencyConfig states an operation no stored dependency is called on",
            json!({ "DependencyConfig": {
                "DependencyKeyAttributes": { "Type": "RemoteService", "Name": "ledger" },
                "DependencyOperationName": "Settle"
            } }),
            vec![],
        ),
    ];

    for (label, filter, expected) in &cases {
        let (status, page) = call(&router, "POST", "/slos", Some(filter.clone())).await;
        assert_eq!(
            status, 200,
            "case {label:?}: expected HTTP 200 from ListServiceLevelObjectives, got {status} with {page}"
        );
        assert_eq!(
            names(&page),
            expected.iter().map(|name| (*name).to_string()).collect::<Vec<_>>(),
            "case {label:?}: expected {expected:?}, got {:?}",
            names(&page)
        );
    }
}

/// The additional attributes travel with the stored record, so a selector that narrows by them
/// is matched against what a read of the objective answers.
#[tokio::test]
async fn the_stored_source_carries_the_additional_attributes_the_filter_reads() {
    let router = router();
    seed(&router).await;

    let (status, fetched) = call(&router, "GET", "/slo/ms-rum-checkout", None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 reading the objective back, got {status} with {fetched}"
    );
    let source = &fetched["Slo"]["Sli"]["SliMetric"]["MetricSource"];
    for (key, value) in [("Page", "checkout"), ("Team", "storefront")] {
        assert_eq!(
            source["MetricSourceAttributes"][key].as_str(),
            Some(value),
            "expected the stored MetricSourceAttributes.{key} to be {value:?}, got {:?}",
            source["MetricSourceAttributes"]
        );
    }
    assert_eq!(
        source["MetricSourceKeyAttributes"]["Name"].as_str(),
        Some("web"),
        "expected the stored MetricSourceKeyAttributes to name the app monitor, got {:?}",
        source["MetricSourceKeyAttributes"]
    );

    let (status, bare) = call(&router, "GET", "/slo/ms-rum-bare", None).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 reading the attribute-free objective back, got {status} with {bare}"
    );
    assert!(
        bare["Slo"]["Sli"]["SliMetric"]["MetricSource"]["MetricSourceAttributes"].is_null(),
        "expected the source stored without additional attributes to report none, got {:?}",
        bare["Slo"]["Sli"]["SliMetric"]["MetricSource"]
    );
}

/// `MetricSourceAttributes` is an `Attributes` map, so a selector naming a map outside its
/// published `length 1..4` is refused rather than silently narrowing nothing — each bound is
/// accepted above and one step past each is refused here.
#[tokio::test]
async fn additional_attributes_outside_the_modelled_length_are_refused() {
    let router = router();
    seed(&router).await;

    let cases: Vec<(&str, Value, &str)> = vec![
        (
            "no entry, below the modelled minimum of one",
            json!({}),
            "length greater than or equal to 1",
        ),
        (
            "five entries, one past the modelled maximum of four",
            json!({
                "Page": "checkout",
                "Team": "storefront",
                "Tier": "gold",
                "Region": "eu",
                "Cell": "one"
            }),
            "length less than or equal to 4",
        ),
    ];

    for (label, additional, constraint) in &cases {
        let (status, error) = call(
            &router,
            "POST",
            "/slos",
            Some(json!({
                "MetricSource": metric_source(APP_MONITOR_TYPE, "web", Some(additional.clone()))
            })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {label:?}: expected HTTP 400 for a map the model refuses, got {status} with {error}"
        );
        assert_eq!(
            error["__type"]
                .as_str()
                .map(|kind| kind.rsplit('#').next_back().unwrap_or(kind)),
            Some("ValidationException"),
            "case {label:?}: expected a ValidationException, got {error}"
        );
        assert!(
            error["message"]
                .as_str()
                .is_some_and(|message| message.contains(constraint)),
            "case {label:?}: expected the refusal to state {constraint:?}, got {error}"
        );
    }
}
