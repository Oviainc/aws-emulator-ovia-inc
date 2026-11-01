// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage, over HTTP, of the ONE identity each END of a recorded edge is
//! reported and dimensioned by: `ListServiceDependencies` and `ListServiceDependents`, the
//! two views of the same edge.
//!
//! The rule under test: an entity is identified by as few as one of its up to four
//! `Attributes` entries, so two SLOs measuring one edge can name an END of it by identities
//! of different fullness — it is still ONE entity and ONE edge, so each view reports that
//! end under the single fullest identity the store holds for it, reports the edge once, and
//! carries the same `MetricReferences` as the other view. A reported identity is therefore
//! never a strict subset of another identity reported for the same end, and it always
//! carries the minimum identity the `KeyAttributes` member documents for its `Type`, so a
//! client can read the far end back with `ListServiceDependents`.
//!
//! The rows below span what that generalises over: the far end identified as a `Resource`,
//! an `AWS::Resource` and a `RemoteService` (so the fuller record adds the one attribute its
//! `Type` leaves optional); the fuller record stored under the SLO name that sorts first and
//! under the one that sorts last; one edge recorded twice, one far entity reached from two
//! operations, two far entities that are NOT one entity (differing in the optional attribute
//! and in the identifying one), a far end recorded only once, and an edge whose BOTH ends are
//! recorded under identities of different fullness.
//!
//! Every read drives the real `restJson1` surface the router production mounts, so the bodies
//! asserted here are the ones a client sees. The harness (the router, the request sender, the
//! clock, the SLO create and the rendering of an answer's references) comes from the suite
//! that owns the reference rule across the five topology reads, so this crate's tests state
//! it ONCE.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::discovered_metric_references::{
    call, create_slo, dimension_set, dimensions_of, metric_set, now_seconds, router,
    standard_metrics,
};

/// The identity of a NAMED entity: `Name` is applicable to `Service`, `RemoteService` and
/// `AWS::Service`, and `Environment` — applicable to every type — is what a record may leave
/// out (`smithy_query(query="doc", shape="ServiceDependency", member="DependencyKeyAttributes")`).
fn named_entity(entity_type: &str, name: &str, environment: Option<&str>) -> Value {
    let mut entity = json!({ "Type": entity_type, "Name": name });
    if let Some(environment) = environment {
        entity["Environment"] = json!(environment);
    }
    entity
}

/// The identity of an entity whose `Type` says it is a RESOURCE: picked out by its
/// `ResourceType` and `Identifier`, with `Environment` again the attribute a record may leave
/// out (same member doc as [`named_entity`]).
fn resource_entity(
    entity_type: &str,
    resource_type: &str,
    identifier: &str,
    environment: Option<&str>,
) -> Value {
    let mut entity = json!({
        "Type": entity_type,
        "ResourceType": resource_type,
        "Identifier": identifier
    });
    if let Some(environment) = environment {
        entity["Environment"] = json!(environment);
    }
    entity
}

/// One SLO to seed: the entity its indicator measures, the operation of that entity, and the
/// edge it records out of that operation.
struct Seed {
    slo: &'static str,
    caller: Value,
    operation: &'static str,
    /// The far end AS RECORDED by this SLO, which may name the entity by fewer attributes
    /// than another record of the same edge does.
    callee: Value,
    callee_operation: &'static str,
}

/// One edge both views must report, and the dimensions both must carry for it.
struct Edge {
    /// `OperationName` of the `ServiceDependency` entry — the operation of the caller.
    operation: &'static str,
    /// The identity the far end must be reported under: the fullest the records hold for it.
    callee: Value,
    callee_operation: &'static str,
    /// The dimension set every reference of this edge carries, in both views. The dimension
    /// names are the ones `src/state.rs` maps the `Attributes` entries onto
    /// (`NEAR_END_DIMENSIONS`, `FAR_END_DIMENSIONS`, `OPERATION_DIMENSION`,
    /// `REMOTE_OPERATION_DIMENSION`).
    dimensions: &'static [&'static str],
}

/// One caller, the SLOs the store holds for its edges, and what both views must report.
struct Row {
    case: &'static str,
    /// The identity the dependency-direction read names the caller with, carrying the
    /// documented minimum for its `Type`.
    request: Value,
    /// The identity the caller itself must be reported and dimensioned by: the fullest the
    /// SLOs hold for it.
    caller_identity: Value,
    seeds: Vec<Seed>,
    edges: Vec<Edge>,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a resource far end recorded without and with Environment, the fuller \
                   record under the SLO name that sorts last",
            request: named_entity("Service", "checkout", Some("prod")),
            caller_identity: named_entity("Service", "checkout", Some("prod")),
            seeds: vec![
                Seed {
                    slo: "a-checkout-carts",
                    caller: named_entity("Service", "checkout", Some("prod")),
                    operation: "POST /checkout",
                    callee: resource_entity("Resource", "AWS::DynamoDB::Table", "carts", None),
                    callee_operation: "GetItem",
                },
                Seed {
                    slo: "z-checkout-carts",
                    caller: named_entity("Service", "checkout", Some("prod")),
                    operation: "POST /checkout",
                    callee: resource_entity(
                        "Resource",
                        "AWS::DynamoDB::Table",
                        "carts",
                        Some("prod"),
                    ),
                    callee_operation: "GetItem",
                },
            ],
            edges: vec![Edge {
                operation: "POST /checkout",
                callee: resource_entity(
                    "Resource",
                    "AWS::DynamoDB::Table",
                    "carts",
                    Some("prod"),
                ),
                callee_operation: "GetItem",
                dimensions: &[
                    "Environment=prod",
                    "Service=checkout",
                    "Operation=POST /checkout",
                    "RemoteResourceType=AWS::DynamoDB::Table",
                    "RemoteResourceIdentifier=carts",
                    "RemoteOperation=GetItem",
                ],
            }],
        },
        Row {
            case: "the same, with the fuller record under the SLO name that sorts first",
            request: named_entity("Service", "orders", Some("prod")),
            caller_identity: named_entity("Service", "orders", Some("prod")),
            seeds: vec![
                Seed {
                    slo: "a-orders-table",
                    caller: named_entity("Service", "orders", Some("prod")),
                    operation: "POST /orders",
                    callee: resource_entity(
                        "Resource",
                        "AWS::DynamoDB::Table",
                        "orders",
                        Some("prod"),
                    ),
                    callee_operation: "PutItem",
                },
                Seed {
                    slo: "z-orders-table",
                    caller: named_entity("Service", "orders", Some("prod")),
                    operation: "POST /orders",
                    callee: resource_entity("Resource", "AWS::DynamoDB::Table", "orders", None),
                    callee_operation: "PutItem",
                },
            ],
            edges: vec![Edge {
                operation: "POST /orders",
                callee: resource_entity(
                    "Resource",
                    "AWS::DynamoDB::Table",
                    "orders",
                    Some("prod"),
                ),
                callee_operation: "PutItem",
                dimensions: &[
                    "Environment=prod",
                    "Service=orders",
                    "Operation=POST /orders",
                    "RemoteResourceType=AWS::DynamoDB::Table",
                    "RemoteResourceIdentifier=orders",
                    "RemoteOperation=PutItem",
                ],
            }],
        },
        Row {
            // The subset record here names only `Type` and `Name`, which is BELOW the minimum
            // identity a `ListServiceDependents` request must carry, so reporting the edge
            // under it would hand the caller a far end it cannot read back.
            case: "a named far end recorded without and with Environment",
            request: named_entity("Service", "billing", Some("staging")),
            caller_identity: named_entity("Service", "billing", Some("staging")),
            seeds: vec![
                Seed {
                    slo: "a-billing-tax",
                    caller: named_entity("Service", "billing", Some("staging")),
                    operation: "POST /invoice",
                    callee: named_entity("RemoteService", "tax", None),
                    callee_operation: "Quote",
                },
                Seed {
                    slo: "z-billing-tax",
                    caller: named_entity("Service", "billing", Some("staging")),
                    operation: "POST /invoice",
                    callee: named_entity("RemoteService", "tax", Some("prod")),
                    callee_operation: "Quote",
                },
            ],
            edges: vec![Edge {
                operation: "POST /invoice",
                callee: named_entity("RemoteService", "tax", Some("prod")),
                callee_operation: "Quote",
                dimensions: &[
                    "Environment=staging",
                    "Service=billing",
                    "Operation=POST /invoice",
                    "RemoteService=tax",
                    "RemoteOperation=Quote",
                ],
            }],
        },
        Row {
            case: "one far entity reached from two operations, each record naming it \
                   differently",
            request: named_entity("Service", "payments", Some("prod")),
            caller_identity: named_entity("Service", "payments", Some("prod")),
            seeds: vec![
                Seed {
                    slo: "payments-charge",
                    caller: named_entity("Service", "payments", Some("prod")),
                    operation: "POST /charge",
                    callee: resource_entity("AWS::Resource", "AWS::S3::Bucket", "receipts", None),
                    callee_operation: "PutObject",
                },
                Seed {
                    slo: "payments-receipt",
                    caller: named_entity("Service", "payments", Some("prod")),
                    operation: "GET /receipt",
                    callee: resource_entity(
                        "AWS::Resource",
                        "AWS::S3::Bucket",
                        "receipts",
                        Some("prod"),
                    ),
                    callee_operation: "GetObject",
                },
            ],
            edges: vec![
                Edge {
                    operation: "GET /receipt",
                    callee: resource_entity(
                        "AWS::Resource",
                        "AWS::S3::Bucket",
                        "receipts",
                        Some("prod"),
                    ),
                    callee_operation: "GetObject",
                    dimensions: &[
                        "Environment=prod",
                        "Service=payments",
                        "Operation=GET /receipt",
                        "RemoteResourceType=AWS::S3::Bucket",
                        "RemoteResourceIdentifier=receipts",
                        "RemoteOperation=GetObject",
                    ],
                },
                Edge {
                    operation: "POST /charge",
                    callee: resource_entity(
                        "AWS::Resource",
                        "AWS::S3::Bucket",
                        "receipts",
                        Some("prod"),
                    ),
                    callee_operation: "PutObject",
                    dimensions: &[
                        "Environment=prod",
                        "Service=payments",
                        "Operation=POST /charge",
                        "RemoteResourceType=AWS::S3::Bucket",
                        "RemoteResourceIdentifier=receipts",
                        "RemoteOperation=PutObject",
                    ],
                },
            ],
        },
        Row {
            // Neither of the two resource identities holds every entry of the other, so they
            // name two entities rather than one, and the edges into them are two edges.
            case: "two far ends that are not one entity: one differing in the optional \
                   attribute, one in the identifying attribute",
            request: named_entity("Service", "search", Some("prod")),
            caller_identity: named_entity("Service", "search", Some("prod")),
            seeds: vec![
                Seed {
                    slo: "search-index-prod",
                    caller: named_entity("Service", "search", Some("prod")),
                    operation: "POST /query",
                    callee: resource_entity(
                        "Resource",
                        "AWS::OpenSearch::Domain",
                        "index",
                        Some("prod"),
                    ),
                    callee_operation: "Search",
                },
                Seed {
                    slo: "search-index-staging",
                    caller: named_entity("Service", "search", Some("prod")),
                    operation: "POST /query",
                    callee: resource_entity(
                        "Resource",
                        "AWS::OpenSearch::Domain",
                        "index",
                        Some("staging"),
                    ),
                    callee_operation: "Search",
                },
                Seed {
                    slo: "search-ranker",
                    caller: named_entity("Service", "search", Some("prod")),
                    operation: "POST /query",
                    callee: named_entity("RemoteService", "ranker", Some("prod")),
                    callee_operation: "Rank",
                },
            ],
            edges: vec![
                Edge {
                    operation: "POST /query",
                    callee: resource_entity(
                        "Resource",
                        "AWS::OpenSearch::Domain",
                        "index",
                        Some("prod"),
                    ),
                    callee_operation: "Search",
                    dimensions: &[
                        "Environment=prod",
                        "Service=search",
                        "Operation=POST /query",
                        "RemoteResourceType=AWS::OpenSearch::Domain",
                        "RemoteResourceIdentifier=index",
                        "RemoteOperation=Search",
                    ],
                },
                Edge {
                    operation: "POST /query",
                    callee: resource_entity(
                        "Resource",
                        "AWS::OpenSearch::Domain",
                        "index",
                        Some("staging"),
                    ),
                    callee_operation: "Search",
                    // The catalogue names no environment dimension for the far end of a call,
                    // so these two edges differ in their reported identity rather than in
                    // their dimensions.
                    dimensions: &[
                        "Environment=prod",
                        "Service=search",
                        "Operation=POST /query",
                        "RemoteResourceType=AWS::OpenSearch::Domain",
                        "RemoteResourceIdentifier=index",
                        "RemoteOperation=Search",
                    ],
                },
                Edge {
                    operation: "POST /query",
                    callee: named_entity("RemoteService", "ranker", Some("prod")),
                    callee_operation: "Rank",
                    dimensions: &[
                        "Environment=prod",
                        "Service=search",
                        "Operation=POST /query",
                        "RemoteService=ranker",
                        "RemoteOperation=Rank",
                    ],
                },
            ],
        },
        Row {
            case: "a far end recorded only once, under the identity the record names",
            request: named_entity("Service", "support", Some("prod")),
            caller_identity: named_entity("Service", "support", Some("prod")),
            seeds: vec![Seed {
                slo: "support-crm",
                caller: named_entity("Service", "support", Some("prod")),
                operation: "GET /ticket",
                callee: named_entity("RemoteService", "crm", Some("prod")),
                callee_operation: "Lookup",
            }],
            edges: vec![Edge {
                operation: "GET /ticket",
                callee: named_entity("RemoteService", "crm", Some("prod")),
                callee_operation: "Lookup",
                dimensions: &[
                    "Environment=prod",
                    "Service=support",
                    "Operation=GET /ticket",
                    "RemoteService=crm",
                    "RemoteOperation=Lookup",
                ],
            }],
        },
        Row {
            case: "an edge whose both ends are recorded under identities of different \
                   fullness",
            request: resource_entity("Resource", "AWS::S3::Bucket", "assets", None),
            caller_identity: resource_entity(
                "Resource",
                "AWS::S3::Bucket",
                "assets",
                Some("prod"),
            ),
            seeds: vec![
                Seed {
                    slo: "a-assets-get",
                    caller: resource_entity("Resource", "AWS::S3::Bucket", "assets", None),
                    operation: "GetObject",
                    callee: named_entity("RemoteService", "thumbnailer", None),
                    callee_operation: "Resize",
                },
                Seed {
                    slo: "z-assets-get",
                    caller: resource_entity(
                        "Resource",
                        "AWS::S3::Bucket",
                        "assets",
                        Some("prod"),
                    ),
                    operation: "GetObject",
                    callee: named_entity("RemoteService", "thumbnailer", Some("prod")),
                    callee_operation: "Resize",
                },
            ],
            edges: vec![Edge {
                operation: "GetObject",
                callee: named_entity("RemoteService", "thumbnailer", Some("prod")),
                callee_operation: "Resize",
                dimensions: &[
                    "Environment=prod",
                    "ResourceType=AWS::S3::Bucket",
                    "ResourceIdentifier=assets",
                    "Operation=GetObject",
                    "RemoteService=thumbnailer",
                    "RemoteOperation=Resize",
                ],
            }],
        },
    ]
}

/// Store one row's SLOs on a fresh router, so each row reads back only its own edges.
async fn seeded(row: &Row) -> MultiServiceRouter {
    let router = router();
    for seed in &row.seeds {
        create_slo(
            &router,
            seed.slo,
            json!({
                "Name": seed.slo,
                "SliConfig": { "SliMetricConfig": {
                    "KeyAttributes": seed.caller.clone(),
                    "OperationName": seed.operation,
                    "MetricType": "LATENCY",
                    "DependencyConfig": {
                        "DependencyKeyAttributes": seed.callee.clone(),
                        "DependencyOperationName": seed.callee_operation
                    }
                }}
            }),
        )
        .await;
    }
    router
}

/// The window every read here covers: the seeded SLOs are created inside it.
fn covering_window() -> String {
    let now = now_seconds();
    format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600)
}

/// Read one topology operation of `entity` over `window`, asserting it is served.
async fn read(router: &MultiServiceRouter, case: &str, path: &str, entity: &Value) -> Value {
    let window = covering_window();
    let (status, answer) = call(
        router,
        "POST",
        &format!("{path}?{window}"),
        Some(json!({ "KeyAttributes": entity.clone() })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} on {path} for {entity:?}: expected HTTP 200, got {status} with {answer:?}"
    );
    answer
}

/// The entries of one topology answer.
fn entries(answer: &Value, member: &str) -> Vec<Value> {
    answer[member].as_array().cloned().unwrap_or_default()
}

/// The `Name=Value` entries of one `Attributes` map, so one identity can be compared with
/// another as a SET — which is what says whether it names the same entity.
fn attribute_pairs(attributes: &Value) -> BTreeSet<String> {
    attributes
        .as_object()
        .map(|map| {
            map.iter()
                .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
                .collect()
        })
        .unwrap_or_default()
}

/// `OperationName|DependencyOperationName` of one edge: the pair of operations that, with the
/// two ends, identifies it.
fn operation_pair(operation: &str, callee_operation: &str) -> String {
    format!("{operation}|{callee_operation}")
}

/// Assert that `entry`'s references are the standard catalogue, every one dimensioned by
/// `expected`.
fn assert_references(case: &str, entry: &Value, expected: &BTreeSet<String>) {
    assert_eq!(
        metric_set(&entry["MetricReferences"]),
        standard_metrics(),
        "case {case:?}: expected the standard metrics, got {:?}",
        entry["MetricReferences"]
    );
    for reference in entry["MetricReferences"]
        .as_array()
        .cloned()
        .unwrap_or_default()
    {
        assert_eq!(
            &dimensions_of(&reference),
            expected,
            "case {case:?}: expected the edge's own dimensions on {:?}, got {:?}",
            reference["MetricName"],
            reference["Dimensions"]
        );
    }
}

/// `ListServiceDependencies` reports one entry per recorded edge, its far end under the one
/// identity the store holds for that entity — never under a strict subset of an identity
/// another record of the same edge names it by.
#[tokio::test]
async fn one_edge_is_reported_once_with_its_far_end_under_one_identity() {
    for row in rows() {
        let router = seeded(&row).await;
        let answer = read(&router, row.case, "/service-dependencies", &row.request).await;
        let reported = entries(&answer, "ServiceDependencies");

        assert_eq!(
            reported.len(),
            row.edges.len(),
            "case {:?}: expected {} edges, got {:?}",
            row.case,
            row.edges.len(),
            answer["ServiceDependencies"]
        );

        for edge in &row.edges {
            let wanted = operation_pair(edge.operation, edge.callee_operation);
            let matched: Vec<&Value> = reported
                .iter()
                .filter(|entry| {
                    operation_pair(
                        entry["OperationName"].as_str().unwrap_or_default(),
                        entry["DependencyOperationName"].as_str().unwrap_or_default(),
                    ) == wanted
                        && entry["DependencyKeyAttributes"] == edge.callee
                })
                .collect();
            assert_eq!(
                matched.len(),
                1,
                "case {:?}, edge {wanted:?}: expected exactly one entry whose far end is \
                 {:?}, got {:?}",
                row.case,
                edge.callee,
                answer["ServiceDependencies"]
            );
            assert_references(row.case, matched[0], &dimension_set(edge.dimensions));
        }

        // One entity is never reported under two identities: an identity that holds every
        // entry of another names the same entity, so a reported far end that is a strict
        // subset of another reported far end would be that entity listed twice.
        for outer in &reported {
            for inner in &reported {
                let (wider, narrower) = (
                    attribute_pairs(&outer["DependencyKeyAttributes"]),
                    attribute_pairs(&inner["DependencyKeyAttributes"]),
                );
                assert!(
                    wider == narrower || !narrower.is_subset(&wider),
                    "case {:?}: expected one far entity under one identity, got {narrower:?} \
                     beside {wider:?}",
                    row.case
                );
            }
        }
    }
}

/// `ListServiceDependents`, read on the far end a dependency entry reports, answers for that
/// edge: it serves the identity, reports the edge once, names the dependent by the identity
/// the caller-side view dimensions it by, and carries the very same references.
#[tokio::test]
async fn both_views_of_an_edge_report_one_reference_set() {
    for row in rows() {
        let router = seeded(&row).await;
        let answer = read(&router, row.case, "/service-dependencies", &row.request).await;
        let reported = entries(&answer, "ServiceDependencies");

        // How many of this caller's edges reach each far entity — every one of them must come
        // back from the read on that entity, and nothing else.
        let mut expected_per_callee: BTreeMap<String, usize> = BTreeMap::new();
        for entry in &reported {
            *expected_per_callee
                .entry(format!("{:?}", attribute_pairs(&entry["DependencyKeyAttributes"])))
                .or_default() += 1;
        }

        for entry in &reported {
            let callee = entry["DependencyKeyAttributes"].clone();
            let wanted = operation_pair(
                entry["OperationName"].as_str().unwrap_or_default(),
                entry["DependencyOperationName"].as_str().unwrap_or_default(),
            );
            let inverse = read(&router, row.case, "/service-dependents", &callee).await;
            let dependents = entries(&inverse, "ServiceDependents");
            let expected = expected_per_callee
                .get(&format!("{:?}", attribute_pairs(&callee)))
                .copied()
                .unwrap_or_default();
            assert_eq!(
                dependents.len(),
                expected,
                "case {:?}, far end {callee:?}: expected the {expected} edge(s) into it, got \
                 {:?}",
                row.case,
                inverse["ServiceDependents"]
            );

            // `OperationName` on a dependent is the operation of the named entity that was
            // invoked, and `DependentOperationName` the operation of the dependent that
            // invoked it — so the pair is the caller-side pair, read the other way round.
            let matched: Vec<&Value> = dependents
                .iter()
                .filter(|dependent| {
                    operation_pair(
                        dependent["DependentOperationName"].as_str().unwrap_or_default(),
                        dependent["OperationName"].as_str().unwrap_or_default(),
                    ) == wanted
                })
                .collect();
            assert_eq!(
                matched.len(),
                1,
                "case {:?}, edge {wanted:?} into {callee:?}: expected the far end to report \
                 this edge once, got {:?}",
                row.case,
                inverse["ServiceDependents"]
            );
            assert_eq!(
                matched[0]["DependentKeyAttributes"], row.caller_identity,
                "case {:?}, edge {wanted:?}: expected the dependent under the identity the \
                 caller-side view dimensions it by, got {:?}",
                row.case, matched[0]["DependentKeyAttributes"]
            );
            assert_eq!(
                matched[0]["MetricReferences"], entry["MetricReferences"],
                "case {:?}, edge {wanted:?}: expected both views to report one reference set, \
                 got {:?} and {:?}",
                row.case, matched[0]["MetricReferences"], entry["MetricReferences"]
            );
        }
    }
}
