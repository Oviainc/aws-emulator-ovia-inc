// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the dimensions a discovered entity's `MetricReferences` carry when
//! the entity is identified as a RESOURCE rather than named, over the reads that report a
//! whole entity: `ListServices` and `GetService`.
//!
//! The rule under test: each reference of a reported entity is dimensioned by one dimension
//! per key attribute that the entity's own `Type` makes applicable to its identity and the
//! stored record holds — the resource pair (`ResourceType`, `Identifier`) for a resource-typed
//! entity exactly as the name for a named one, the environment for either — and a reference
//! with no dimension to state omits the `Dimensions` member rather than reporting an empty
//! array.
//!
//! Every read drives the real `restJson1` surface the router production mounts, so the bodies
//! asserted here are the ones a client sees. The harness is the one
//! `discovered_metric_references` writes out for this crate's reference suites.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::discovered_metric_references::{
    call, create_slo, dimension_set, dimensions_of, metric_set, now_seconds, router,
    standard_metrics,
};

/// One seeded entity, and the dimensions every metric reference reported for it must carry.
///
/// The rows span what the rule generalises over: the `Type` that identifies an entity as a
/// resource (`Resource`, `AWS::Resource`) and the ones that name it (`Service`,
/// `RemoteService`), the kind of resource (a table, a log group, a queue), two resources of
/// ONE kind in ONE environment differing only in which one they are, an identity that names
/// no `Environment`, an identity carrying an attribute its own `Type` makes inapplicable (a
/// `Name` on a resource, an `Identifier` on a service), and an identity that states nothing
/// beyond the kind of object it is.
///
/// The attribute-per-`Type` split is the `KeyAttributes` member's own
/// (`smithy_query(query="doc", shape="ServiceSummary", member="KeyAttributes")`): `Name` is
/// "used only if the value of the `Type` field is `Service`, `RemoteService`, or
/// `AWS::Service`", `ResourceType` and `Identifier` "only when the value of the `Type` field
/// is `Resource` or `AWS::Resource`", and `Environment` is restricted to no type.
struct Row {
    case: &'static str,
    /// The SLO name the entity is seeded under.
    slo: &'static str,
    /// The `KeyAttributes` the SLO records the entity under, which `ListServices` reports it
    /// by.
    entity: Value,
    /// The dimension set every reference of the entity carries, or `None` when the record
    /// states no dimension at all and the member must be ABSENT.
    expected: Option<&'static [&'static str]>,
}

/// The rows, each seeded as one SLO's entity and read back.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a DynamoDB table",
            slo: "orders-table-latency",
            entity: json!({
                "Type": "Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "orders-table",
                "Environment": "prod"
            }),
            expected: Some(&[
                "Environment=prod",
                "ResourceType=AWS::DynamoDB::Table",
                "ResourceIdentifier=orders-table",
            ]),
        },
        Row {
            case: "a second table of the same kind in the same environment",
            slo: "customers-table-latency",
            entity: json!({
                "Type": "Resource",
                "ResourceType": "AWS::DynamoDB::Table",
                "Identifier": "customers-table",
                "Environment": "prod"
            }),
            expected: Some(&[
                "Environment=prod",
                "ResourceType=AWS::DynamoDB::Table",
                "ResourceIdentifier=customers-table",
            ]),
        },
        Row {
            case: "an AWS::Resource log group that names no Environment",
            slo: "log-group-latency",
            entity: json!({
                "Type": "AWS::Resource",
                "ResourceType": "AWS::Logs::LogGroup",
                "Identifier": "/aws/lambda/x"
            }),
            expected: Some(&[
                "ResourceType=AWS::Logs::LogGroup",
                "ResourceIdentifier=/aws/lambda/x",
            ]),
        },
        Row {
            case: "a resource that also carries a Name, which its Type makes inapplicable",
            slo: "orders-queue-latency",
            entity: json!({
                "Type": "Resource",
                "ResourceType": "AWS::SQS::Queue",
                "Identifier": "orders-queue",
                "Name": "orders-queue-label"
            }),
            expected: Some(&[
                "ResourceType=AWS::SQS::Queue",
                "ResourceIdentifier=orders-queue",
            ]),
        },
        Row {
            case: "a service that also carries an Identifier, which its Type makes inapplicable",
            slo: "checkout-latency",
            entity: json!({
                "Type": "Service",
                "Name": "checkout",
                "Environment": "prod",
                "Identifier": "checkout-identifier"
            }),
            expected: Some(&["Environment=prod", "Service=checkout"]),
        },
        Row {
            case: "a named service, dimensioned by the name its Type does make applicable",
            slo: "billing-latency",
            entity: json!({ "Type": "Service", "Name": "billing", "Environment": "prod" }),
            expected: Some(&["Environment=prod", "Service=billing"]),
        },
        Row {
            case: "a resource whose only other attribute its Type makes inapplicable",
            slo: "unnamed-resource-latency",
            entity: json!({ "Type": "Resource", "Name": "unnamed-resource" }),
            expected: None,
        },
        Row {
            case: "an identity that states only the kind of object it is",
            slo: "bare-remote-service-latency",
            entity: json!({ "Type": "RemoteService" }),
            expected: None,
        },
    ]
}

/// Seed one SLO per row, which is this emulator's evidence that the entity was discovered.
async fn seed(router: &MultiServiceRouter) {
    for row in rows() {
        create_slo(
            router,
            row.case,
            json!({
                "Name": row.slo,
                "SliConfig": { "SliMetricConfig": {
                    "KeyAttributes": row.entity,
                    "MetricType": "LATENCY"
                }},
                "CreateRecommendedSlo": false
            }),
        )
        .await;
    }
}

/// A window covering the instant the seeded SLOs are created at.
fn covering_window() -> String {
    let now = now_seconds();
    format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600)
}

/// Every summary `ListServices` reports over the covering window.
async fn listed_services(router: &MultiServiceRouter) -> Vec<Value> {
    let (status, answer) = call(
        router,
        "GET",
        &format!("/services?{}", covering_window()),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "ListServices: expected HTTP 200, got {status} with {answer:?}"
    );
    answer["ServiceSummaries"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| panic!("ListServices: ServiceSummaries is @required, got {answer:?}"))
}

/// The references one entry reports, as the wire carries them.
fn references_of(entry: &Value) -> Vec<Value> {
    entry["MetricReferences"].as_array().cloned().unwrap_or_default()
}

/// Assert `entry` carries the standard metrics, each dimensioned by `expected` — or each with
/// no `Dimensions` member at all when the record states no dimension.
fn assert_dimensioned(case: &str, entry: &Value, expected: Option<&[&str]>) {
    assert_eq!(
        metric_set(&entry["MetricReferences"]),
        standard_metrics(),
        "case {case:?}: expected the standard metrics, got {:?}",
        entry["MetricReferences"]
    );
    let references = references_of(entry);
    assert!(
        !references.is_empty(),
        "case {case:?}: expected a discovered entity to carry metrics, got {entry:?}"
    );
    for reference in &references {
        match expected {
            Some(expected) => assert_eq!(
                dimensions_of(reference),
                dimension_set(expected),
                "case {case:?}: expected the entity's own dimensions on {:?}, got {:?}",
                reference["MetricName"],
                reference["Dimensions"]
            ),
            None => assert!(
                reference.get("Dimensions").is_none(),
                "case {case:?}: expected no Dimensions member on {:?}, got {:?}",
                reference["MetricName"],
                reference["Dimensions"]
            ),
        }
    }
}

/// Every reported entity is dimensioned by the attributes its own `Type` identifies it by,
/// and `GetService` answers for the same entity the same way.
#[tokio::test]
async fn an_entity_is_dimensioned_by_the_attributes_its_type_identifies_it_by() {
    let router = router();
    seed(&router).await;
    let summaries = listed_services(&router).await;

    for row in rows() {
        let summary = summaries
            .iter()
            .find(|summary| summary["KeyAttributes"] == row.entity)
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: expected the seeded identity {:?} among {summaries:?}",
                    row.case, row.entity
                )
            });
        assert_dimensioned(row.case, summary, row.expected);

        // The same entity read back through `GetService`, for every row whose identity that
        // read serves: it reports the entity by one identity, so its references cannot differ
        // from the listing's for the same entity. A row whose record does not carry the pair
        // its `Type` is read by is refused there rather than served, which `entity_identity`
        // pins.
        let (status, answer) = call(
            &router,
            "POST",
            &format!("/service?{}", covering_window()),
            Some(json!({ "KeyAttributes": row.entity.clone() })),
        )
        .await;
        if status != 200 {
            assert!(
                row.expected.is_none(),
                "case {:?}: expected GetService to serve the identity, got {status} with {answer:?}",
                row.case
            );
            continue;
        }
        assert_eq!(
            answer["Service"]["MetricReferences"], summary["MetricReferences"],
            "case {:?}: expected GetService to report the listing's references, got {:?} and {:?}",
            row.case, answer["Service"]["MetricReferences"], summary["MetricReferences"]
        );
        assert_dimensioned(row.case, &answer["Service"], row.expected);
    }
}

/// Two entities whose stored identities differ are reported with references that differ: the
/// dimension set states what the record states, so two resources of one kind in one
/// environment are told apart by the identifier that picks each of them out.
#[tokio::test]
async fn entities_with_different_identities_carry_different_dimensions() {
    let router = router();
    seed(&router).await;
    let summaries = listed_services(&router).await;

    let mut seen: BTreeSet<BTreeSet<String>> = BTreeSet::new();
    for summary in &summaries {
        let references = references_of(summary);
        let dimensions = references
            .first()
            .map(dimensions_of)
            .unwrap_or_else(|| panic!("expected a reported summary to carry metrics: {summary:?}"));
        // An identity that states no dimension states the same nothing as any other such
        // identity, so only the entities with something to state are told apart here.
        if dimensions.is_empty() {
            continue;
        }
        assert!(
            seen.insert(dimensions.clone()),
            "identity {:?}: expected each identity's own dimensions, got {dimensions:?} twice",
            summary["KeyAttributes"]
        );
    }
    assert_eq!(
        seen.len(),
        rows().iter().filter(|row| row.expected.is_some()).count(),
        "expected one dimension set per identity with something to state, got {seen:?}"
    );
}

/// No read reports a reference with a present-but-empty `Dimensions`: the member holds "one or
/// more dimensions" (`smithy_query(query="shape", shape="MetricReference")`), so a reference
/// with nothing to state omits it.
///
/// The rows differ along the read, and so along the kind of entry: a whole entity, an
/// operation of one, an edge out of one and the same edge seen from its far end.
#[tokio::test]
async fn no_reference_reports_an_empty_dimension_list() {
    let router = router();
    seed(&router).await;
    // One more SLO, so the operation and edge reads report an entry for a resource-typed
    // caller too.
    let caller = json!({
        "Type": "AWS::Resource",
        "ResourceType": "AWS::Lambda::Function",
        "Identifier": "ship-fn",
        "Environment": "prod"
    });
    let callee = json!({ "Type": "RemoteService", "Name": "rates", "Environment": "prod" });
    create_slo(
        &router,
        "a resource caller with an operation and an edge",
        json!({
            "Name": "ship-fn-rates",
            "SliConfig": { "SliMetricConfig": {
                "KeyAttributes": caller.clone(),
                "OperationName": "Invoke",
                "MetricType": "LATENCY",
                "DependencyConfig": {
                    "DependencyKeyAttributes": callee.clone(),
                    "DependencyOperationName": "Quote"
                }
            }},
            "CreateRecommendedSlo": false
        }),
    )
    .await;

    let window = covering_window();
    let reads: Vec<(&str, &str, &str, Option<Value>, &str)> = vec![
        ("ListServices", "GET", "/services", None, "ServiceSummaries"),
        (
            "ListServiceOperations of a resource caller",
            "POST",
            "/service-operations",
            Some(json!({ "KeyAttributes": caller.clone() })),
            "ServiceOperations",
        ),
        (
            "ListServiceDependencies of a resource caller",
            "POST",
            "/service-dependencies",
            Some(json!({ "KeyAttributes": caller.clone() })),
            "ServiceDependencies",
        ),
        (
            "ListServiceDependents of the far end",
            "POST",
            "/service-dependents",
            Some(json!({ "KeyAttributes": callee.clone() })),
            "ServiceDependents",
        ),
    ];

    for (case, method, path, body, member) in reads {
        let (status, answer) = call(&router, method, &format!("{path}?{window}"), body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        let entries = answer[member].as_array().cloned().unwrap_or_default();
        assert!(
            !entries.is_empty(),
            "case {case:?}: expected the read to report an entry, got {answer:?}"
        );
        for entry in &entries {
            for reference in references_of(entry) {
                assert!(
                    reference["Dimensions"].as_array().is_none_or(|dimensions| {
                        !dimensions.is_empty()
                    }),
                    "case {case:?}: expected a stated or an absent dimension list, got {reference:?}"
                );
            }
        }
    }

    // The resource caller's own operation and edge state its identity, so the near end of
    // both carries the pair that picks the resource out beside the environment it runs in.
    let (_, operations) = call(
        &router,
        "POST",
        &format!("/service-operations?{window}"),
        Some(json!({ "KeyAttributes": caller })),
    )
    .await;
    let expected = dimension_set(&[
        "Environment=prod",
        "ResourceType=AWS::Lambda::Function",
        "ResourceIdentifier=ship-fn",
        "Operation=Invoke",
    ]);
    let references = references_of(&operations["ServiceOperations"][0]);
    for reference in &references {
        assert_eq!(
            dimensions_of(reference),
            expected,
            "case {:?}: expected the operation's own dimensions, got {:?}",
            "the operation of a resource caller",
            reference["Dimensions"]
        );
    }
}
