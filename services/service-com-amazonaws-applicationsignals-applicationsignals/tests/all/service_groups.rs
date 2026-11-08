// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `ServiceGroups` a reported service carries: the groups the
//! account's stored grouping configuration puts a discovered service in.
//!
//! The rule under test: once a grouping configuration is stored, every reported discovered
//! service carries one group per grouping attribute definition that gives it a value —
//! derived from the service's own attributes under one of the definition's source keys,
//! otherwise the definition's declared default — and none for a definition that gives it no
//! value. The groups are the same whichever read reports the service and on whichever page
//! `ListServices` reports it, and nothing is grouped while no configuration is stored.
//!
//! Every test drives the real `restJson1` surface through the router that production mounts,
//! so the routes, the status codes and the JSON bodies are the ones a client sees.

use std::collections::BTreeSet;
use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The SigV4 credential scope routes the request to this service. Signature verification is
/// off (see `set_request_authentication`), so the signature bytes are never read.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/eu-west-2/application-signals/aws4_request, \
     SignedHeaders=host, Signature=0000";

/// The region in [`AUTHORIZATION`].
const REGION: &str = "eu-west-2";

/// The longest value a reported group may carry: `GroupValue` is modelled `length 1..255`
/// (`smithy_query(query="shape",shape="GroupValue")`).
const MAX_GROUP_VALUE: usize = 255;

/// The longest a `GroupIdentifier` may be (`smithy_query(query="shape",shape="GroupIdentifier")`:
/// `length 1..1024`).
const MAX_GROUP_IDENTIFIER: usize = 1024;

/// A router with just this service mounted on a fresh engine.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine),
    );
    router
}

/// Send one request and return its status and parsed JSON body (`Value::Null` when empty).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map_or_else(Vec::new, |body| {
        serde_json::to_vec(&body).expect("serialize the request body")
    });
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://application-signals.{REGION}.amazonaws.com{uri}"
        ))
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(payload))
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let body = response.into_body();
    let parsed = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body).unwrap_or(Value::Null)
    };
    (status, parsed)
}

/// Seconds since the epoch, so a window can be placed around the instant a stored SLO is
/// created at.
fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs() as i64
}

/// The window every read below asks for: an hour either side of now, so a record created now
/// falls inside it.
fn window() -> String {
    let now = now_seconds();
    format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600)
}

/// Create one SLO whose indicator monitors `key_attributes` — this emulator's evidence that
/// Application Signals discovered that service.
async fn create_slo(router: &MultiServiceRouter, case: &str, name: &str, key_attributes: &Value) {
    let (status, answer) = call(
        router,
        "POST",
        "/slo",
        Some(json!({
            "Name": name,
            "SliConfig": {
                "SliMetricConfig": { "KeyAttributes": key_attributes, "MetricType": "LATENCY" }
            }
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the SLO create to answer 200, got {status} with {answer}"
    );
}

/// Store `definitions` as the account's grouping configuration.
async fn put_grouping(router: &MultiServiceRouter, case: &str, definitions: &Value) {
    let (status, answer) = call(
        router,
        "PUT",
        "/grouping-configuration",
        Some(json!({ "GroupingAttributeDefinitions": definitions })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the grouping-configuration put to answer 200, got {status} \
         with {answer}"
    );
}

/// `GetService` over the window, for `key_attributes`.
async fn get_service(router: &MultiServiceRouter, case: &str, key_attributes: &Value) -> Value {
    let (status, body) = call(
        router,
        "POST",
        &format!("/service?{}", window()),
        Some(json!({ "KeyAttributes": key_attributes })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected GetService to answer 200, got {status} with {body}"
    );
    body
}

/// `ListServices` over the window.
async fn list_services(router: &MultiServiceRouter, case: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/services?{}", window()), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected ListServices to answer 200, got {status} with {body}"
    );
    body
}

/// The `(GroupName, GroupValue, GroupSource)` triples a reported service carries, in the
/// order reported. An absent `ServiceGroups` member reports no triples at all.
fn reported_groups(service: &Value) -> Vec<(String, String, String)> {
    service
        .get("ServiceGroups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .map(|group| {
                    (
                        group["GroupName"].as_str().unwrap_or_default().to_string(),
                        group["GroupValue"].as_str().unwrap_or_default().to_string(),
                        group["GroupSource"].as_str().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The `GroupIdentifier`s a reported service carries, in the order reported.
fn reported_identifiers(service: &Value) -> Vec<String> {
    service
        .get("ServiceGroups")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .map(|group| {
                    group["GroupIdentifier"]
                        .as_str()
                        .unwrap_or_default()
                        .to_string()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// An expected group, as a triple the response is compared against.
fn group(name: &str, value: &str, source: &str) -> (String, String, String) {
    (name.to_string(), value.to_string(), source.to_string())
}

/// One row of the grouping table.
struct Case {
    label: &'static str,
    /// The `GroupingAttributeDefinitions` the configuration holds.
    definitions: Value,
    /// The entity a stored SLO monitors, and the identity the read asks for.
    entity: Value,
    /// The `(GroupName, GroupValue, GroupSource)` triples the read must report, in order.
    expected: Vec<(String, String, String)>,
}

/// A printable-ASCII attribute value of `length` characters, for the published `GroupValue`
/// bound and the case one past it. `KeyAttributeValue` is `length 1..1024`
/// (`smithy_query(query="shape",shape="Attributes")`), so both fit in an attribute.
fn long_value(length: usize) -> String {
    "c".repeat(length)
}

/// The rows the rule is asserted over.
///
/// They differ along what the rule generalises over: how many definitions the configuration
/// holds, whether a definition declares source keys, a default, both or neither, how many
/// source keys it lists and which of them the entity carries, the CASING a source key names
/// an attribute in, WHICH attribute of the entity a value is derived from (`Environment`,
/// `Name`, `Type`, `Identifier`), the entity's own size (three attributes and the modelled
/// maximum of four), the length of the derived value against the published `GroupValue`
/// maximum of 255 and one character past it (as the only key and as the first of two), and
/// whether two attribute names differ only in ASCII case.
fn cases() -> Vec<Case> {
    let service = json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" });
    let resource = json!({
        "Type": "AWS::Resource",
        "ResourceType": "AWS::DynamoDB::Table",
        "Identifier": "orders-table",
        "Environment": "eks:prod/default"
    });

    vec![
        Case {
            label: "a default and no source keys: every service carries the default",
            definitions: json!([{
                "GroupingName": "BusinessUnit",
                "DefaultGroupingValue": "Unassigned"
            }]),
            entity: service.clone(),
            expected: vec![group("BusinessUnit", "Unassigned", "DEFAULT")],
        },
        Case {
            label: "a source key the entity carries: the value is derived from it",
            definitions: json!([{
                "GroupingName": "Tier",
                "GroupingSourceKeys": ["Environment"],
                "DefaultGroupingValue": "Unassigned"
            }]),
            entity: service.clone(),
            expected: vec![group("Tier", "eks:prod/default", "OTEL")],
        },
        Case {
            label: "the source key names the attribute in another casing",
            definitions: json!([{
                "GroupingName": "Tier",
                "GroupingSourceKeys": ["environment"],
                "DefaultGroupingValue": "Unassigned"
            }]),
            entity: service.clone(),
            expected: vec![group("Tier", "eks:prod/default", "OTEL")],
        },
        Case {
            label: "source keys the entity carries none of: the default stands in",
            definitions: json!([{
                "GroupingName": "BusinessUnit",
                "GroupingSourceKeys": ["business_unit", "team"],
                "DefaultGroupingValue": "Unassigned"
            }]),
            entity: service.clone(),
            expected: vec![group("BusinessUnit", "Unassigned", "DEFAULT")],
        },
        Case {
            label: "no source key matches and no default is declared: no group",
            definitions: json!([{
                "GroupingName": "BusinessUnit",
                "GroupingSourceKeys": ["business_unit"]
            }]),
            entity: service.clone(),
            expected: Vec::new(),
        },
        Case {
            label: "neither source keys nor a default: no group",
            definitions: json!([{ "GroupingName": "BusinessUnit" }]),
            entity: service.clone(),
            expected: Vec::new(),
        },
        Case {
            label: "the first source key the entity carries wins",
            definitions: json!([{
                "GroupingName": "Entry",
                "GroupingSourceKeys": ["unknown_key", "Name", "Environment"]
            }]),
            entity: service.clone(),
            expected: vec![group("Entry", "checkout", "OTEL")],
        },
        Case {
            label: "source keys in the other order derive the other attribute",
            definitions: json!([{
                "GroupingName": "Entry",
                "GroupingSourceKeys": ["Environment", "Name"]
            }]),
            entity: service.clone(),
            expected: vec![group("Entry", "eks:prod/default", "OTEL")],
        },
        Case {
            label: "three definitions: one group each, in configuration order",
            definitions: json!([
                {
                    "GroupingName": "BusinessUnit",
                    "GroupingSourceKeys": ["business_unit"],
                    "DefaultGroupingValue": "Unassigned"
                },
                { "GroupingName": "Platform", "GroupingSourceKeys": ["Type"] },
                { "GroupingName": "Owner", "DefaultGroupingValue": "platform-team" }
            ]),
            entity: service.clone(),
            expected: vec![
                group("BusinessUnit", "Unassigned", "DEFAULT"),
                group("Platform", "Service", "OTEL"),
                group("Owner", "platform-team", "DEFAULT"),
            ],
        },
        Case {
            label: "only the definitions that give a value report a group",
            definitions: json!([
                { "GroupingName": "BusinessUnit", "GroupingSourceKeys": ["business_unit"] },
                { "GroupingName": "Owner", "DefaultGroupingValue": "platform-team" }
            ]),
            entity: resource.clone(),
            expected: vec![group("Owner", "platform-team", "DEFAULT")],
        },
        Case {
            label: "a four-attribute entity derives from an attribute only it carries",
            definitions: json!([{
                "GroupingName": "Store",
                "GroupingSourceKeys": ["Identifier"],
                "DefaultGroupingValue": "Unassigned"
            }]),
            entity: resource,
            expected: vec![group("Store", "orders-table", "OTEL")],
        },
        Case {
            label: "a derived value at the published GroupValue maximum is reported",
            definitions: json!([{
                "GroupingName": "Entry",
                "GroupingSourceKeys": ["Name"],
                "DefaultGroupingValue": "Unassigned"
            }]),
            entity: json!({
                "Type": "Service",
                "Name": long_value(MAX_GROUP_VALUE),
                "Environment": "eks:prod/default"
            }),
            expected: vec![group("Entry", &long_value(MAX_GROUP_VALUE), "OTEL")],
        },
        Case {
            label: "a derived value one past that maximum falls back to the default",
            definitions: json!([{
                "GroupingName": "Entry",
                "GroupingSourceKeys": ["Name"],
                "DefaultGroupingValue": "Unassigned"
            }]),
            entity: json!({
                "Type": "Service",
                "Name": long_value(MAX_GROUP_VALUE + 1),
                "Environment": "eks:prod/default"
            }),
            expected: vec![group("Entry", "Unassigned", "DEFAULT")],
        },
        Case {
            label: "a later source key supplies the value when the first is too long",
            definitions: json!([{
                "GroupingName": "Entry",
                "GroupingSourceKeys": ["Name", "Environment"],
                "DefaultGroupingValue": "Unassigned"
            }]),
            entity: json!({
                "Type": "Service",
                "Name": long_value(MAX_GROUP_VALUE + 1),
                "Environment": "eks:prod/default"
            }),
            expected: vec![group("Entry", "eks:prod/default", "OTEL")],
        },
        // Two attribute names that differ only in ASCII case, which `KeyAttributeName`
        // (`pattern ^[a-zA-Z]{1,50}$`) admits: the exactly-named attribute is the one a
        // source key selects, and when the key matches neither exactly the smaller name by
        // byte order is taken — never whichever entry a map walk reaches first.
        Case {
            label: "the exactly-named attribute wins over one matching only in case",
            definitions: json!([{ "GroupingName": "Entry", "GroupingSourceKeys": ["name"] }]),
            entity: json!({
                "Type": "Service",
                "Name": "checkout",
                "name": "shadow",
                "Environment": "eks:prod/default"
            }),
            expected: vec![group("Entry", "shadow", "OTEL")],
        },
        Case {
            label: "a source key matching two attributes only in case takes the smaller name",
            definitions: json!([{ "GroupingName": "Entry", "GroupingSourceKeys": ["NAME"] }]),
            entity: json!({
                "Type": "Service",
                "Name": "checkout",
                "name": "shadow",
                "Environment": "eks:prod/default"
            }),
            expected: vec![group("Entry", "checkout", "OTEL")],
        },
    ]
}

/// A discovered service carries one group per grouping attribute definition that gives it a
/// value, and `ListServices` reports the same groups for the same service as `GetService`.
#[tokio::test]
async fn a_discovered_service_carries_the_groups_the_configuration_gives_it() {
    for case in cases() {
        let router = router();
        create_slo(&router, case.label, "checkout-latency", &case.entity).await;
        put_grouping(&router, case.label, &case.definitions).await;

        let body = get_service(&router, case.label, &case.entity).await;
        let service = &body["Service"];
        assert_eq!(
            reported_groups(service),
            case.expected,
            "case {:?}: expected service groups {:?}, got {:?}",
            case.label,
            case.expected,
            service.get("ServiceGroups")
        );

        // An identifier is "a unique identifier for this grouping attribute value", so it
        // states the pair it belongs to: within one response no two groups share one, and
        // every identifier fits the modelled length.
        let identifiers = reported_identifiers(service);
        let distinct: BTreeSet<&String> = identifiers.iter().collect();
        assert_eq!(
            distinct.len(),
            identifiers.len(),
            "case {:?}: expected one distinct GroupIdentifier per group, got {identifiers:?}",
            case.label
        );
        for identifier in &identifiers {
            assert!(
                !identifier.is_empty() && identifier.chars().count() <= MAX_GROUP_IDENTIFIER,
                "case {:?}: expected a GroupIdentifier within length 1..{MAX_GROUP_IDENTIFIER}, \
                 got {identifier:?} ({} characters)",
                case.label,
                identifier.chars().count()
            );
        }

        // The groups are a fact about the service, not about the read that reported it.
        let listed = list_services(&router, case.label).await;
        let summaries = listed["ServiceSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let summary = summaries
            .iter()
            .find(|summary| summary["KeyAttributes"] == service["KeyAttributes"])
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected ListServices to report the discovered service, got {:?}",
                    case.label, listed["ServiceSummaries"]
                )
            });
        assert_eq!(
            reported_groups(summary),
            case.expected,
            "case {:?}: expected ListServices to report the same groups, got {:?}",
            case.label,
            summary.get("ServiceGroups")
        );
        assert_eq!(
            reported_identifiers(summary),
            identifiers,
            "case {:?}: expected ListServices to report the same identifiers, got {:?}",
            case.label,
            summary.get("ServiceGroups")
        );

        // A read is repeatable: the same request reports the same groups.
        let again = get_service(&router, case.label, &case.entity).await;
        assert_eq!(
            again["Service"], body["Service"],
            "case {:?}: two reads of one service reported different groups",
            case.label
        );
    }
}

/// `ListServices` reports each discovered service's OWN groups, on whichever page that
/// service falls: a definition over an attribute every service carries gives each one the
/// value IT holds, and a definition over an attribute only one of them carries groups only
/// that one. Paging does not change what a summary carries, and every page names the metrics
/// of the services on it.
///
/// The walk spans the page size — 1, 2, and a size past the number of discovered services,
/// within the modelled `ListServicesMaxResults` range of 1..100 — over three services whose
/// identities differ in size (three attributes and the modelled maximum of four), in the
/// attributes they carry, and in the value they hold under the attribute the first
/// definition reads.
#[tokio::test]
async fn listed_services_carry_their_own_groups_on_every_page() {
    let router = router();
    let services = [
        (
            "checkout-latency",
            json!({ "Type": "Service", "Name": "checkout", "Environment": "prod" }),
            vec![group("Stage", "prod", "OTEL")],
        ),
        (
            "orders-latency",
            json!({ "Type": "Service", "Name": "orders", "Environment": "staging" }),
            vec![group("Stage", "staging", "OTEL")],
        ),
        (
            "orders-table-latency",
            json!({
                "Type": "AWS::Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "prod"
            }),
            vec![
                group("Stage", "prod", "OTEL"),
                group("Store", "orders-table", "OTEL"),
            ],
        ),
    ];

    for (name, entity, _) in &services {
        create_slo(&router, name, name, entity).await;
    }
    put_grouping(
        &router,
        "a grouping over one shared and one service-specific attribute",
        &json!([
            {
                "GroupingName": "Stage",
                "GroupingSourceKeys": ["Environment"],
                "DefaultGroupingValue": "Unassigned"
            },
            { "GroupingName": "Store", "GroupingSourceKeys": ["Identifier"] }
        ]),
    )
    .await;

    for page_size in [1_usize, 2, 100] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut token: Option<String> = None;
        // ONE window for the whole walk, as a client paging a report asks for: the cursor a
        // page hands back names a position in the read THAT page served, so a later page
        // asked for over a different window is a different read and the cursor is not its
        // to resume. `window()` is computed from the clock, so re-deriving it per request
        // moved the window whenever the second ticked between two pages.
        let window = window();
        for _ in 0..services.len() + 2 {
            let uri = match token.as_deref() {
                Some(token) => format!(
                    "/services?{window}&MaxResults={page_size}&NextToken={token}"
                ),
                None => format!("/services?{window}&MaxResults={page_size}"),
            };
            let (status, answer) = call(&router, "GET", &uri, None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with {answer}"
            );
            // The page BOUND and the cursor's once-each walk are
            // `list_services::pages_are_bounded_and_the_cursor_walks_every_service_once`'s
            // rule; what is asserted here is what each summary CARRIES, on each page.
            let summaries = answer["ServiceSummaries"]
                .as_array()
                .cloned()
                .unwrap_or_default();

            for summary in &summaries {
                let (name, _, expected) = services
                    .iter()
                    .find(|(_, entity, _)| &summary["KeyAttributes"] == entity)
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: unexpected summary {summary}");
                    });
                assert_eq!(
                    reported_groups(summary),
                    *expected,
                    "case {case:?}, service {name:?}: expected its own groups {expected:?}, got {:?}",
                    summary.get("ServiceGroups")
                );
                // The issue this suite's rule answers also had every listed service naming
                // no metric at all, so a page's summaries are asserted to name theirs.
                let references = summary["MetricReferences"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                assert!(
                    !references.is_empty(),
                    "case {case:?}, service {name:?}: expected the service's metrics, got {:?}",
                    summary["MetricReferences"]
                );
                for reference in &references {
                    assert!(
                        ["Namespace", "MetricName", "MetricType"].iter().all(|member| {
                            reference[*member].as_str().is_some_and(|value| !value.is_empty())
                        }),
                        "case {case:?}, service {name:?}: expected a fully named metric, got {reference:?}"
                    );
                }
                walked.push(summary["KeyAttributes"].clone());
            }

            token = answer["NextToken"].as_str().map(ToString::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: expected the walk to end without a NextToken, got {token:?}"
        );
        // Not a vacuous pass: the walk must have reached every service, so every row's
        // groups were actually asserted.
        for (name, entity, _) in &services {
            assert!(
                walked.iter().any(|walked| walked == entity),
                "case {case:?}: expected the walk to reach {name:?}, got {walked:?}"
            );
        }
    }
}

/// The groups follow the STORED configuration: nothing is grouped before a put, the groups
/// change when the configuration is replaced, and nothing is grouped once it is deleted.
#[tokio::test]
async fn the_reported_groups_follow_the_stored_configuration() {
    let case = "the configuration is written, replaced and deleted";
    let router = router();
    let entity = json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" });
    create_slo(&router, case, "checkout-latency", &entity).await;

    let before = get_service(&router, case, &entity).await;
    assert_eq!(
        reported_groups(&before["Service"]),
        Vec::new(),
        "case {case:?}: expected no groups while no configuration is stored, got {:?}",
        before["Service"].get("ServiceGroups")
    );

    put_grouping(
        &router,
        case,
        &json!([{ "GroupingName": "BusinessUnit", "DefaultGroupingValue": "Unassigned" }]),
    )
    .await;
    let first = get_service(&router, case, &entity).await;
    assert_eq!(
        reported_groups(&first["Service"]),
        vec![group("BusinessUnit", "Unassigned", "DEFAULT")],
        "case {case:?}: expected the stored definition's group, got {:?}",
        first["Service"].get("ServiceGroups")
    );

    // A second put REPLACES the configuration, so the groups are the new definitions'.
    put_grouping(
        &router,
        case,
        &json!([{ "GroupingName": "Tier", "GroupingSourceKeys": ["Environment"] }]),
    )
    .await;
    let replaced = get_service(&router, case, &entity).await;
    assert_eq!(
        reported_groups(&replaced["Service"]),
        vec![group("Tier", "eks:prod/default", "OTEL")],
        "case {case:?}: expected the replacing definition's group, got {:?}",
        replaced["Service"].get("ServiceGroups")
    );

    let (status, answer) = call(&router, "DELETE", "/grouping-configuration", None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the grouping-configuration delete to answer 200, got {status} \
         with {answer}"
    );
    let after = get_service(&router, case, &entity).await;
    assert_eq!(
        reported_groups(&after["Service"]),
        Vec::new(),
        "case {case:?}: expected no groups once the configuration is deleted, got {:?}",
        after["Service"].get("ServiceGroups")
    );
    assert!(
        after["Service"].get("ServiceGroups").is_none(),
        "case {case:?}: expected ServiceGroups to be absent rather than empty, got {:?}",
        after["Service"]["ServiceGroups"]
    );
}

/// Only a DISCOVERED service is grouped: an identity no stored record is evidence of carries
/// no groups and no metric references, however the grouping configuration is written, and
/// neither does a discovered service read over a window it was not observed in.
#[tokio::test]
async fn an_identity_nothing_was_observed_for_carries_no_groups() {
    let monitored = json!({ "Type": "Service", "Name": "checkout", "Environment": "eks:prod/default" });
    let definitions = json!([
        { "GroupingName": "BusinessUnit", "DefaultGroupingValue": "Unassigned" },
        { "GroupingName": "Tier", "GroupingSourceKeys": ["Environment"] }
    ]);

    struct Case {
        label: &'static str,
        /// Whether a stored SLO monitors the entity the read asks for.
        seed: bool,
        /// The identity the read asks for.
        request: Value,
        /// The window the read asks over, relative to now, in seconds.
        offsets: (i64, i64),
    }

    let cases = vec![
        Case {
            label: "no record monitors the identity",
            seed: false,
            request: json!({ "Type": "Service", "Name": "typo", "Environment": "eks:prod/default" }),
            offsets: (-3_600, 3_600),
        },
        Case {
            label: "a record monitors another entity entirely",
            seed: true,
            request: json!({ "Type": "Service", "Name": "typo", "Environment": "eks:prod/default" }),
            offsets: (-3_600, 3_600),
        },
        Case {
            label: "the window ends before the record existed",
            seed: true,
            request: monitored.clone(),
            offsets: (-7_200, -3_600),
        },
        Case {
            label: "the window has not begun",
            seed: true,
            request: monitored.clone(),
            offsets: (3_600, 7_200),
        },
    ];

    for case in cases {
        let router = router();
        if case.seed {
            create_slo(&router, case.label, "checkout-latency", &monitored).await;
        }
        put_grouping(&router, case.label, &definitions).await;

        let now = now_seconds();
        let (status, body) = call(
            &router,
            "POST",
            &format!(
                "/service?StartTime={}&EndTime={}",
                now + case.offsets.0,
                now + case.offsets.1
            ),
            Some(json!({ "KeyAttributes": case.request })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {:?}: expected GetService to answer 200, got {status} with {body}",
            case.label
        );
        assert_eq!(
            reported_groups(&body["Service"]),
            Vec::new(),
            "case {:?}: expected no groups for an identity nothing was observed for, got {:?}",
            case.label,
            body["Service"].get("ServiceGroups")
        );
        assert_eq!(
            body["Service"]["MetricReferences"].as_array().map(Vec::len),
            Some(0),
            "case {:?}: expected no metric references either, got {:?}",
            case.label,
            body["Service"]["MetricReferences"]
        );
    }
}
