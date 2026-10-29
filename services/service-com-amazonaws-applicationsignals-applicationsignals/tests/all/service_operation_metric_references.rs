// SPDX-License-Identifier: CC0-1.0

//! End-to-end coverage of the `MetricReferences` `ListServiceOperations` reports over HTTP,
//! for every kind of entity the store can be evidence of an operation on.
//!
//! The rule under test: every reported `ServiceOperation` carries the standard metrics
//! Application Signals collects for THAT operation — `Latency`, `Fault` and `Error` in the
//! `ApplicationSignals` namespace — dimensioned by the entity's stored identity plus the
//! operation, so no reported operation carries an empty list, two operations of one service
//! carry different dimensions, and a dimension whose attribute the stored identity does not
//! carry is left out rather than reported empty. The paged view reports the same references
//! as the single-page one, since a page is a window onto the same projection.
//!
//! Every read drives the real `restJson1` surface the router production mounts, so the
//! bodies asserted here are the ones a client sees. (The harness below is this file's own,
//! as in every other suite in this crate; the expected metric catalogue comes from the
//! suite that owns that rule, so it is written out once for the crate.)

use std::collections::BTreeSet;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The harness (the router, the request sender, the clock and the SLO create), the expected
// metric catalogue and the rendering of one answer's references and dimensions all come from
// the suite that owns this rule across the five topology reads, so this crate's tests state
// them ONCE.
use super::discovered_metric_references::{
    call, create_slo, dimension_set, dimensions_of, metric_set, now_seconds, router,
    standard_metrics,
};

/// A walk over one entity's operations cannot exceed this; a cursor that does is looping.
const EXPECTED_OPERATION_CEILING: usize = 32;

/// The window every read here covers: the stored SLOs are created inside it.
fn covering_window() -> String {
    let now = now_seconds();
    format!("StartTime={}&EndTime={}", now - 3_600, now + 3_600)
}

/// A seeded SLO: the entity members under the SLI form `request_based` selects, so the same
/// entity and operation can be recorded either way.
fn slo_body(name: &str, request_based: bool, metric_config: Value) -> Value {
    if request_based {
        json!({
            "Name": name,
            "RequestBasedSliConfig": { "RequestBasedSliMetricConfig": metric_config }
        })
    } else {
        json!({ "Name": name, "SliConfig": { "SliMetricConfig": metric_config } })
    }
}

/// The identity of an entity whose `Type` says it is NAMED (`Service`, `RemoteService` or
/// `AWS::Service`).
fn named_entity(entity_type: &str, name: &str, environment: &str) -> Value {
    json!({ "Type": entity_type, "Name": name, "Environment": environment })
}

/// The identity of an entity whose `Type` says it is a RESOURCE, picked out by its
/// `ResourceType` and `Identifier` rather than by a `Name`.
fn resource_entity(resource_type: &str, identifier: &str, environment: Option<&str>) -> Value {
    let mut entity = json!({
        "Type": "Resource",
        "ResourceType": resource_type,
        "Identifier": identifier
    });
    if let Some(environment) = environment {
        entity["Environment"] = json!(environment);
    }
    entity
}

/// One operation the store is evidence of: the SLO that records it, and the dimension set
/// every metric reference of that operation must carry.
struct Seeded {
    /// The `Name` of the SLO recording it; distinct per seeded SLO.
    slo: &'static str,
    /// The entity the SLO's indicator names, which is also what a read names it by.
    entity: Value,
    /// `OperationName` of the SLO's indicator, or `None` for an SLO that names no
    /// operation of the entity — which is evidence of no operation at all.
    operation: Option<&'static str>,
    /// Whether the entity members are recorded under `RequestBasedSliConfig`.
    request_based: bool,
    /// The `MetricType` the SLO measures, which does not narrow the catalogue a discovered
    /// operation reports.
    metric_type: &'static str,
}

/// Every SLO the tests here read back.
///
/// They span what the rule generalises over: several operations of ONE entity, one
/// operation recorded by TWO SLOs, the `SliConfig` and the `RequestBasedSliConfig` form of
/// the same members, either `MetricType` an Application Signals indicator may measure, each
/// `Type` of entity the member documents (a service, a remote service, an Amazon Web
/// Services service and a resource), an identity of three attributes and one of four, a
/// resource identity that names no `Name` at all and one that also names no `Environment`,
/// a second environment, operation names of different forms, and an entity whose SLO names
/// no operation.
fn seeded() -> Vec<Seeded> {
    vec![
        Seeded {
            slo: "checkout-checkout-latency",
            entity: named_entity("Service", "checkout", "prod"),
            operation: Some("POST /checkout"),
            request_based: false,
            metric_type: "LATENCY",
        },
        Seeded {
            // A SECOND SLO on the same operation of the same entity: one operation, so one
            // reported entry.
            slo: "checkout-checkout-availability",
            entity: named_entity("Service", "checkout", "prod"),
            operation: Some("POST /checkout"),
            request_based: true,
            metric_type: "AVAILABILITY",
        },
        Seeded {
            slo: "checkout-cart-availability",
            entity: named_entity("Service", "checkout", "prod"),
            operation: Some("GET /cart"),
            request_based: true,
            metric_type: "AVAILABILITY",
        },
        Seeded {
            slo: "checkout-cart-update",
            entity: named_entity("Service", "checkout", "prod"),
            operation: Some("PUT /cart/{id}"),
            request_based: false,
            metric_type: "LATENCY",
        },
        Seeded {
            slo: "orders-orders-latency",
            entity: named_entity("Service", "orders", "staging"),
            operation: Some("POST /orders"),
            request_based: false,
            metric_type: "LATENCY",
        },
        Seeded {
            slo: "payments-charge-latency",
            entity: named_entity("RemoteService", "payments", "prod"),
            operation: Some("Charge"),
            request_based: true,
            metric_type: "LATENCY",
        },
        Seeded {
            slo: "sqs-send-latency",
            entity: named_entity("AWS::Service", "AWS::SQS", "prod"),
            operation: Some("SendMessage"),
            request_based: false,
            metric_type: "LATENCY",
        },
        Seeded {
            slo: "ship-fn-invoke-latency",
            entity: resource_entity("AWS::Lambda::Function", "ship-fn", Some("prod")),
            operation: Some("Invoke"),
            request_based: false,
            metric_type: "LATENCY",
        },
        Seeded {
            slo: "assets-get-latency",
            entity: resource_entity("AWS::S3::Bucket", "assets", None),
            operation: Some("GetObject"),
            request_based: true,
            metric_type: "LATENCY",
        },
        Seeded {
            slo: "support-availability",
            entity: named_entity("Service", "support", "prod"),
            operation: None,
            request_based: false,
            metric_type: "AVAILABILITY",
        },
    ]
}

/// Store every SLO [`seeded`] names.
async fn seed(router: &MultiServiceRouter) {
    for slo in seeded() {
        let mut metric_config = json!({
            "KeyAttributes": slo.entity.clone(),
            "MetricType": slo.metric_type
        });
        if let Some(operation) = slo.operation {
            metric_config["OperationName"] = json!(operation);
        }
        create_slo(
            router,
            slo.slo,
            slo_body(slo.slo, slo.request_based, metric_config),
        )
        .await;
    }
}

/// One read: the entity it names, and the operations it must report with the dimension set
/// each one's references carry.
///
/// The dimension names are the ones `src/state.rs` maps the `Attributes` entries onto
/// (`NEAR_END_DIMENSIONS` and `OPERATION_DIMENSION`): an entity contributes the service it
/// is and the environment it runs in, plus the operation the metrics were collected for. A
/// resource identity names no `Name`, so it contributes the resource pair that identifies it
/// instead of a `Service` dimension.
struct Row {
    case: &'static str,
    entity: Value,
    /// Operation name and its dimension set, in the order the read reports them (the
    /// operation name's order).
    expected: Vec<(&'static str, &'static [&'static str])>,
}

/// The expected reads, one row per entity the seed is evidence of plus two that it is not.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a service with three operations, one of them recorded twice",
            entity: named_entity("Service", "checkout", "prod"),
            expected: vec![
                (
                    "GET /cart",
                    &["Environment=prod", "Service=checkout", "Operation=GET /cart"],
                ),
                (
                    "POST /checkout",
                    &[
                        "Environment=prod",
                        "Service=checkout",
                        "Operation=POST /checkout",
                    ],
                ),
                (
                    "PUT /cart/{id}",
                    &[
                        "Environment=prod",
                        "Service=checkout",
                        "Operation=PUT /cart/{id}",
                    ],
                ),
            ],
        },
        Row {
            case: "a service in a second environment",
            entity: named_entity("Service", "orders", "staging"),
            expected: vec![(
                "POST /orders",
                &[
                    "Environment=staging",
                    "Service=orders",
                    "Operation=POST /orders",
                ],
            )],
        },
        Row {
            case: "a remote service",
            entity: named_entity("RemoteService", "payments", "prod"),
            expected: vec![(
                "Charge",
                &["Environment=prod", "Service=payments", "Operation=Charge"],
            )],
        },
        Row {
            case: "an Amazon Web Services service",
            entity: named_entity("AWS::Service", "AWS::SQS", "prod"),
            expected: vec![(
                "SendMessage",
                &[
                    "Environment=prod",
                    "Service=AWS::SQS",
                    "Operation=SendMessage",
                ],
            )],
        },
        Row {
            case: "a resource identity, which names no Name",
            entity: resource_entity("AWS::Lambda::Function", "ship-fn", Some("prod")),
            expected: vec![(
                "Invoke",
                &[
                    "Environment=prod",
                    "ResourceType=AWS::Lambda::Function",
                    "ResourceIdentifier=ship-fn",
                    "Operation=Invoke",
                ],
            )],
        },
        Row {
            case: "a resource identity that also names no Environment",
            entity: resource_entity("AWS::S3::Bucket", "assets", None),
            expected: vec![(
                "GetObject",
                &[
                    "ResourceType=AWS::S3::Bucket",
                    "ResourceIdentifier=assets",
                    "Operation=GetObject",
                ],
            )],
        },
        Row {
            case: "an entity whose only SLO names no operation",
            entity: named_entity("Service", "support", "prod"),
            expected: Vec::new(),
        },
        Row {
            case: "an identity no SLO monitors",
            entity: named_entity("Service", "ghost", "prod"),
            expected: Vec::new(),
        },
    ]
}

/// Read every operation of `entity` over `window`, following `NextToken` with the page size
/// `max_results` when one is given.
async fn operations_of(
    router: &MultiServiceRouter,
    entity: &Value,
    window: &str,
    max_results: Option<u32>,
) -> Vec<Value> {
    let mut walked = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut uri = format!("/service-operations?{window}");
        if let Some(max_results) = max_results {
            uri.push_str(&format!("&MaxResults={max_results}"));
        }
        if let Some(token) = token.as_deref() {
            uri.push_str(&format!("&NextToken={token}"));
        }
        let (status, answer) = call(
            router,
            "POST",
            &uri,
            Some(json!({ "KeyAttributes": entity.clone() })),
        )
        .await;
        assert_eq!(
            status, 200,
            "entity {entity:?}: expected HTTP 200, got {status} with {answer:?}"
        );
        let page = answer["ServiceOperations"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| {
                panic!("entity {entity:?}: ServiceOperations is @required, got {answer:?}")
            });
        if let Some(max_results) = max_results {
            assert!(
                page.len() <= max_results as usize,
                "entity {entity:?}: expected at most MaxResults={max_results} entries, got {}",
                page.len()
            );
        }
        walked.extend(page);
        token = answer["NextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
        assert!(
            walked.len() <= EXPECTED_OPERATION_CEILING,
            "entity {entity:?}: the cursor did not terminate, walked {} entries",
            walked.len()
        );
    }
    walked
}

/// Every reported operation carries the standard metrics, dimensioned by the entity's stored
/// identity plus that operation.
#[tokio::test]
async fn every_listed_operation_carries_the_metrics_of_its_own_identity() {
    let router = router();
    seed(&router).await;
    let window = covering_window();

    for row in rows() {
        let reported = operations_of(&router, &row.entity, &window, None).await;
        assert_eq!(
            reported
                .iter()
                .map(|entry| entry["Name"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            row.expected
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>(),
            "case {:?}: expected each discovered operation once, in name order, got {reported:?}",
            row.case
        );

        for (entry, (name, dimensions)) in reported.iter().zip(&row.expected) {
            let case = format!("{}, {name}", row.case);
            assert_eq!(
                metric_set(&entry["MetricReferences"]),
                standard_metrics(),
                "case {case:?}: expected the standard metrics, got {:?}",
                entry["MetricReferences"]
            );
            let expected = dimension_set(dimensions);
            for reference in entry["MetricReferences"]
                .as_array()
                .cloned()
                .unwrap_or_default()
            {
                assert_eq!(
                    dimensions_of(&reference),
                    expected,
                    "case {case:?}: expected the operation's own dimensions on {:?}, got {:?}",
                    reference["MetricName"],
                    reference["Dimensions"]
                );
                assert!(
                    reference.get("AccountId").is_none(),
                    "case {case:?}: a same-account metric names no source account, got \
                     {reference:?}"
                );
            }
        }
    }
}

/// No reported operation carries an empty reference list, and two operations of one entity
/// are never dimensioned the same way, so a caller can tell which operation's metrics it is
/// looking at.
#[tokio::test]
async fn two_operations_of_one_entity_carry_different_dimensions() {
    let router = router();
    seed(&router).await;
    let window = covering_window();

    for row in rows() {
        let reported = operations_of(&router, &row.entity, &window, None).await;
        let mut seen: BTreeSet<BTreeSet<String>> = BTreeSet::new();
        for entry in &reported {
            let references = entry["MetricReferences"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert!(
                !references.is_empty(),
                "case {:?}: expected a discovered operation to carry metrics, got {entry:?}",
                row.case
            );
            let dimensions = dimensions_of(&references[0]);
            assert!(
                seen.insert(dimensions.clone()),
                "case {:?}: expected each operation's metrics to be dimensioned differently, \
                 got {dimensions:?} twice",
                row.case
            );
        }
    }
}

/// A page is a window onto the same projection, so walking an entity's operations one page
/// size at a time reports the same entries, references and all, in the same order as a
/// single page.
///
/// The page sizes span the modelled `range 1..100` of `ListServiceOperationMaxResults`
/// (`smithy_query(shape="ListServiceOperationMaxResults")`): both bounds, a size inside it,
/// and one past each bound, which is refused rather than coerced.
#[tokio::test]
async fn a_paged_walk_reports_the_same_references() {
    let router = router();
    seed(&router).await;
    let window = covering_window();
    let entity = named_entity("Service", "checkout", "prod");
    let single_page = operations_of(&router, &entity, &window, Some(100)).await;
    assert_eq!(
        single_page.len(),
        3,
        "expected checkout's three discovered operations on one page, got {single_page:?}"
    );

    for page_size in [1_u32, 2, 100] {
        let walked = operations_of(&router, &entity, &window, Some(page_size)).await;
        assert_eq!(
            walked, single_page,
            "case \"MaxResults={page_size}\": expected the walk to report every operation \
             exactly once with its own references, got {walked:?}"
        );
    }

    for page_size in [0_u32, 101] {
        let (status, answer) = call(
            &router,
            "POST",
            &format!("/service-operations?{window}&MaxResults={page_size}"),
            Some(json!({ "KeyAttributes": entity.clone() })),
        )
        .await;
        assert_eq!(
            status, 400,
            "case \"MaxResults={page_size}\": expected HTTP 400 for a value outside range \
             1..100, got {status} with {answer:?}"
        );
    }
}

/// An operation the store is no evidence of over the requested period is not reported at
/// all, so there is no entry to carry references: the projection is a function of the
/// window, not of the entity alone.
///
/// The rows differ along why there is no evidence: a period that ended before the SLOs
/// existed, and one that has not begun.
#[tokio::test]
async fn an_operation_outside_the_period_is_not_reported() {
    let router = router();
    seed(&router).await;
    let now = now_seconds();
    let entity = named_entity("Service", "checkout", "prod");

    let windows = [
        (
            "a period that ended before the SLOs existed",
            format!("StartTime={}&EndTime={}", now - 7_200, now - 3_600),
        ),
        (
            "a period that has not begun",
            format!("StartTime={}&EndTime={}", now + 3_600, now + 7_200),
        ),
    ];

    for (case, window) in windows {
        let reported = operations_of(&router, &entity, &window, None).await;
        assert!(
            reported.is_empty(),
            "case {case:?}: expected no reported operation, got {reported:?}"
        );
    }
}
