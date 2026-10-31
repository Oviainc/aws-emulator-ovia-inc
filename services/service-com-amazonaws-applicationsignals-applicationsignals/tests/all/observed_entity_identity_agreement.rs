// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the ONE identity every topology read reports an entity by, over
//! HTTP.
//!
//! The rule under test: an entity is identified by as few as one of its up to four
//! `Attributes` entries, so the same entity can be recorded by a subset of the attributes
//! one of its SLOs names and by a fuller set another names — it is still ONE entity, so
//! every read reports it, and dimensions the metrics collected for it, by the single
//! fullest identity the store holds for it. No two entries of one answer and no two reads
//! over one window can therefore disagree about an entity's own identity or dimensions.
//!
//! The rows below span what that generalises over: the two `Type`s whose documented minimum
//! identity leaves `Environment` optional (so two SLOs on one entity CAN record identities
//! of different size), the fuller identity recorded on the operation that sorts first, on
//! the one that sorts last and on an SLO that names no operation at all, two operations and
//! three, an entity every SLO records under one identity, a request that names the fuller
//! identity itself, and edges out of a mixed-identity entity read from both ends.
//!
//! Every read drives the real `restJson1` surface the router production mounts, so the
//! bodies asserted here are the ones a client sees. The harness (the router, the request
//! sender, the clock, the SLO create and the rendering of an answer's references) comes from
//! the suite that owns the reference rule across the five topology reads, so this crate's
//! tests state it ONCE.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::discovered_metric_references::{
    call, create_slo, dimension_set, dimensions_of, metric_set, now_seconds, router,
    standard_metrics,
};

/// The dimension names that state the NEAR end's own identity, i.e. the entity the entry is
/// about: the catalogue `src/state.rs` maps the `Attributes` entries onto
/// (`NEAR_END_DIMENSIONS`), which is everything beside the operation dimension and the
/// `Remote`-prefixed far-end ones.
const ENTITY_DIMENSION_NAMES: [&str; 4] = [
    "Environment",
    "Service",
    "ResourceType",
    "ResourceIdentifier",
];

/// The dimensions of ONE reference that state the entity's own identity, as
/// [`ENTITY_DIMENSION_NAMES`] names them — so the entity part of an operation's or an edge's
/// dimension set can be compared with the entity part of another read's.
fn entity_dimensions(reference: &Value) -> BTreeSet<String> {
    dimensions_of(reference)
        .into_iter()
        .filter(|dimension| {
            let name = dimension.split('=').next().unwrap_or_default();
            ENTITY_DIMENSION_NAMES.contains(&name)
        })
        .collect()
}

/// The identity of an entity whose `Type` says it is a RESOURCE: picked out by its
/// `ResourceType` and `Identifier`, with `Environment` optional, which is what lets two SLOs
/// on one entity record identities of different size.
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

/// The identity of a NAMED entity, for the far end of a recorded edge.
fn named_entity(entity_type: &str, name: &str, environment: &str) -> Value {
    json!({ "Type": entity_type, "Name": name, "Environment": environment })
}

/// One SLO to seed: the entity its indicator records, the operation of that entity it
/// measures, and the edge it records out of that operation.
struct Seed {
    slo: &'static str,
    entity: Value,
    operation: Option<&'static str>,
    /// The far end of the recorded edge: its identity and the operation it serves.
    dependency: Option<(Value, &'static str)>,
}

/// One entity, the SLOs the store holds for it, and what every read of it must report.
struct Row {
    case: &'static str,
    /// The identity the reads name the entity by, which carries the documented minimum for
    /// its `Type`.
    request: Value,
    slos: Vec<Seed>,
    /// The identity every read must report for the entity: the fullest the SLOs hold.
    identity: Value,
    /// The dimensions every read must carry for the entity itself.
    dimensions: &'static [&'static str],
    /// The operations every read must report, in name order.
    operations: &'static [&'static str],
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a resource whose two operations are recorded under identities of \
                   different size",
            request: resource_entity("Resource", "AWS::S3::Bucket", "assets", None),
            slos: vec![
                Seed {
                    slo: "assets-get-latency",
                    entity: resource_entity("Resource", "AWS::S3::Bucket", "assets", None),
                    operation: Some("GetObject"),
                    dependency: None,
                },
                Seed {
                    slo: "assets-put-latency",
                    entity: resource_entity(
                        "Resource",
                        "AWS::S3::Bucket",
                        "assets",
                        Some("prod"),
                    ),
                    operation: Some("PutObject"),
                    dependency: None,
                },
            ],
            identity: resource_entity("Resource", "AWS::S3::Bucket", "assets", Some("prod")),
            dimensions: &[
                "Environment=prod",
                "ResourceType=AWS::S3::Bucket",
                "ResourceIdentifier=assets",
            ],
            operations: &["GetObject", "PutObject"],
        },
        Row {
            case: "three operations of an Amazon Web Services resource, the fuller identity \
                   on the one that sorts first",
            request: resource_entity("AWS::Resource", "AWS::DynamoDB::Table", "orders", None),
            slos: vec![
                Seed {
                    slo: "orders-delete-latency",
                    entity: resource_entity(
                        "AWS::Resource",
                        "AWS::DynamoDB::Table",
                        "orders",
                        Some("staging"),
                    ),
                    operation: Some("DeleteItem"),
                    dependency: None,
                },
                Seed {
                    slo: "orders-put-latency",
                    entity: resource_entity(
                        "AWS::Resource",
                        "AWS::DynamoDB::Table",
                        "orders",
                        None,
                    ),
                    operation: Some("PutItem"),
                    dependency: None,
                },
                Seed {
                    slo: "orders-query-latency",
                    entity: resource_entity(
                        "AWS::Resource",
                        "AWS::DynamoDB::Table",
                        "orders",
                        None,
                    ),
                    operation: Some("Query"),
                    dependency: None,
                },
            ],
            identity: resource_entity(
                "AWS::Resource",
                "AWS::DynamoDB::Table",
                "orders",
                Some("staging"),
            ),
            dimensions: &[
                "Environment=staging",
                "ResourceType=AWS::DynamoDB::Table",
                "ResourceIdentifier=orders",
            ],
            operations: &["DeleteItem", "PutItem", "Query"],
        },
        Row {
            case: "the fuller identity recorded by an SLO that names no operation",
            request: resource_entity("Resource", "AWS::Lambda::Function", "ship-fn", None),
            slos: vec![
                Seed {
                    slo: "ship-fn-availability",
                    entity: resource_entity(
                        "Resource",
                        "AWS::Lambda::Function",
                        "ship-fn",
                        Some("prod"),
                    ),
                    operation: None,
                    dependency: None,
                },
                Seed {
                    slo: "ship-fn-invoke-latency",
                    entity: resource_entity("Resource", "AWS::Lambda::Function", "ship-fn", None),
                    operation: Some("Invoke"),
                    dependency: None,
                },
            ],
            identity: resource_entity(
                "Resource",
                "AWS::Lambda::Function",
                "ship-fn",
                Some("prod"),
            ),
            dimensions: &[
                "Environment=prod",
                "ResourceType=AWS::Lambda::Function",
                "ResourceIdentifier=ship-fn",
            ],
            operations: &["Invoke"],
        },
        Row {
            case: "a resource every SLO records under one identity, which names no \
                   Environment",
            request: resource_entity("Resource", "AWS::SQS::Queue", "jobs", None),
            slos: vec![
                Seed {
                    slo: "jobs-receive-latency",
                    entity: resource_entity("Resource", "AWS::SQS::Queue", "jobs", None),
                    operation: Some("ReceiveMessage"),
                    dependency: None,
                },
                Seed {
                    slo: "jobs-send-latency",
                    entity: resource_entity("Resource", "AWS::SQS::Queue", "jobs", None),
                    operation: Some("SendMessage"),
                    dependency: None,
                },
            ],
            identity: resource_entity("Resource", "AWS::SQS::Queue", "jobs", None),
            dimensions: &["ResourceType=AWS::SQS::Queue", "ResourceIdentifier=jobs"],
            operations: &["ReceiveMessage", "SendMessage"],
        },
        Row {
            case: "a request that names the fuller identity itself, which the smaller \
                   record does not match",
            request: resource_entity("Resource", "AWS::S3::Bucket", "reports", Some("prod")),
            slos: vec![
                Seed {
                    slo: "reports-get-latency",
                    entity: resource_entity(
                        "Resource",
                        "AWS::S3::Bucket",
                        "reports",
                        Some("prod"),
                    ),
                    operation: Some("GetObject"),
                    dependency: None,
                },
                Seed {
                    slo: "reports-put-latency",
                    entity: resource_entity("Resource", "AWS::S3::Bucket", "reports", None),
                    operation: Some("PutObject"),
                    dependency: None,
                },
            ],
            identity: resource_entity("Resource", "AWS::S3::Bucket", "reports", Some("prod")),
            dimensions: &[
                "Environment=prod",
                "ResourceType=AWS::S3::Bucket",
                "ResourceIdentifier=reports",
            ],
            operations: &["GetObject"],
        },
        Row {
            case: "two edges out of one mixed-identity entity",
            request: resource_entity("Resource", "AWS::S3::Bucket", "media", None),
            slos: vec![
                Seed {
                    slo: "media-get-latency",
                    entity: resource_entity("Resource", "AWS::S3::Bucket", "media", None),
                    operation: Some("GetObject"),
                    dependency: Some((named_entity("RemoteService", "thumbnailer", "prod"), "Resize")),
                },
                Seed {
                    slo: "media-put-latency",
                    entity: resource_entity("Resource", "AWS::S3::Bucket", "media", Some("prod")),
                    operation: Some("PutObject"),
                    dependency: Some((named_entity("RemoteService", "scanner", "prod"), "Scan")),
                },
            ],
            identity: resource_entity("Resource", "AWS::S3::Bucket", "media", Some("prod")),
            dimensions: &[
                "Environment=prod",
                "ResourceType=AWS::S3::Bucket",
                "ResourceIdentifier=media",
            ],
            operations: &["GetObject", "PutObject"],
        },
    ]
}

/// Store one row's SLOs on a fresh router, so each row reads back only its own entity.
async fn seeded(row: &Row) -> MultiServiceRouter {
    let router = router();
    for seed in &row.slos {
        let mut metric_config = json!({
            "KeyAttributes": seed.entity.clone(),
            "MetricType": "LATENCY"
        });
        if let Some(operation) = seed.operation {
            metric_config["OperationName"] = json!(operation);
        }
        if let Some((far_end, far_operation)) = seed.dependency.as_ref() {
            metric_config["DependencyConfig"] = json!({
                "DependencyKeyAttributes": far_end.clone(),
                "DependencyOperationName": far_operation
            });
        }
        create_slo(
            &router,
            seed.slo,
            json!({
                "Name": seed.slo,
                "SliConfig": { "SliMetricConfig": metric_config }
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

/// Read one body-carrying topology operation of `entity` over `window`.
async fn read(
    router: &MultiServiceRouter,
    case: &str,
    path: &str,
    entity: &Value,
    window: &str,
) -> Value {
    let (status, answer) = call(
        router,
        "POST",
        &format!("{path}?{window}"),
        Some(json!({ "KeyAttributes": entity.clone() })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} on {path}: expected HTTP 200, got {status} with {answer:?}"
    );
    answer
}

/// The references of one entry, as a list.
fn references(entry: &Value) -> Vec<Value> {
    entry["MetricReferences"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// `GetService`, `ListServices` and `ListServiceOperations` report one entity by the one
/// identity the store holds for it, and dimension every reference they carry for it by that
/// identity.
#[tokio::test]
async fn every_read_reports_one_entity_by_one_identity() {
    for row in rows() {
        let router = seeded(&row).await;
        let window = covering_window();
        let expected = dimension_set(row.dimensions);

        let service = read(&router, row.case, "/service", &row.request, &window).await;
        assert_eq!(
            service["Service"]["KeyAttributes"], row.identity,
            "case {:?}: expected GetService to report the fullest stored identity, got {:?}",
            row.case, service["Service"]["KeyAttributes"]
        );
        let service_references = references(&service["Service"]);
        assert_eq!(
            metric_set(&service["Service"]["MetricReferences"]),
            standard_metrics(),
            "case {:?}: expected the standard metrics on GetService, got {:?}",
            row.case,
            service["Service"]["MetricReferences"]
        );
        for reference in &service_references {
            assert_eq!(
                dimensions_of(reference),
                expected,
                "case {:?}: expected GetService to dimension {:?} by the entity's identity, \
                 got {:?}",
                row.case,
                reference["MetricName"],
                reference["Dimensions"]
            );
        }

        let operations = read(
            &router,
            row.case,
            "/service-operations",
            &row.request,
            &window,
        )
        .await;
        let entries = operations["ServiceOperations"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry["Name"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            row.operations.to_vec(),
            "case {:?}: expected each discovered operation once, in name order, got {:?}",
            row.case,
            operations["ServiceOperations"]
        );
        for entry in &entries {
            let name = entry["Name"].as_str().unwrap_or_default();
            let mut with_operation = expected.clone();
            with_operation.insert(format!("Operation={name}"));
            for reference in &references(entry) {
                assert_eq!(
                    dimensions_of(reference),
                    with_operation,
                    "case {:?}, operation {name:?}: expected the entity's identity plus the \
                     operation on {:?}, got {:?}",
                    row.case,
                    reference["MetricName"],
                    reference["Dimensions"]
                );
                assert_eq!(
                    entity_dimensions(reference),
                    entity_dimensions(&service_references[0]),
                    "case {:?}, operation {name:?}: expected the entity dimensions GetService \
                     reports for the same entity, got {:?}",
                    row.case,
                    reference["Dimensions"]
                );
            }
        }

        let (status, listed) = call(&router, "GET", &format!("/services?{window}"), None).await;
        assert_eq!(
            status, 200,
            "case {:?} on /services: expected HTTP 200, got {status} with {listed:?}",
            row.case
        );
        let summaries = listed["ServiceSummaries"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            summaries
                .iter()
                .map(|summary| summary["KeyAttributes"].clone())
                .collect::<Vec<_>>(),
            vec![row.identity.clone()],
            "case {:?}: expected ListServices to report the entity once, under the identity \
             GetService echoes back, got {:?}",
            row.case,
            listed["ServiceSummaries"]
        );
        for reference in &references(&summaries[0]) {
            assert_eq!(
                dimensions_of(reference),
                expected,
                "case {:?}: expected ListServices to dimension {:?} by the entity's identity, \
                 got {:?}",
                row.case,
                reference["MetricName"],
                reference["Dimensions"]
            );
        }
    }
}

/// Every edge out of one entity carries that entity's own identity on its near end,
/// whichever record the edge was read off and whichever end of the edge reports it — so the
/// dependency view and the dependent view of one edge still agree reference for reference.
#[tokio::test]
async fn both_ends_of_an_edge_report_the_near_entity_by_one_identity() {
    for row in rows() {
        let router = seeded(&row).await;
        let window = covering_window();
        let expected = dimension_set(row.dimensions);
        let recorded_edges = row
            .slos
            .iter()
            .filter(|seed| seed.dependency.is_some())
            .count();

        let dependencies = read(
            &router,
            row.case,
            "/service-dependencies",
            &row.request,
            &window,
        )
        .await;
        let edges = dependencies["ServiceDependencies"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            edges.len(),
            recorded_edges,
            "case {:?}: expected one entry per recorded edge, got {:?}",
            row.case,
            dependencies["ServiceDependencies"]
        );

        for edge in &edges {
            let operation = edge["OperationName"].as_str().unwrap_or_default();
            for reference in &references(edge) {
                assert_eq!(
                    entity_dimensions(reference),
                    expected,
                    "case {:?}, edge out of {operation:?}: expected the calling entity's own \
                     identity on {:?}, got {:?}",
                    row.case,
                    reference["MetricName"],
                    reference["Dimensions"]
                );
            }

            let dependents = read(
                &router,
                row.case,
                "/service-dependents",
                &edge["DependencyKeyAttributes"],
                &window,
            )
            .await;
            let inverse: Vec<Value> = dependents["ServiceDependents"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .filter(|entry| entry["DependentOperationName"] == edge["OperationName"])
                .collect();
            assert_eq!(
                inverse.len(),
                1,
                "case {:?}, edge out of {operation:?}: expected the far end to report this \
                 edge once, got {:?}",
                row.case,
                dependents["ServiceDependents"]
            );
            assert_eq!(
                inverse[0]["DependentKeyAttributes"], row.identity,
                "case {:?}, edge out of {operation:?}: expected the dependent to be reported \
                 by the identity GetService echoes back, got {:?}",
                row.case, inverse[0]["DependentKeyAttributes"]
            );
            assert_eq!(
                inverse[0]["MetricReferences"], edge["MetricReferences"],
                "case {:?}, edge out of {operation:?}: expected both ends to report one \
                 reference set, got {:?} and {:?}",
                row.case, inverse[0]["MetricReferences"], edge["MetricReferences"]
            );
        }
    }
}
