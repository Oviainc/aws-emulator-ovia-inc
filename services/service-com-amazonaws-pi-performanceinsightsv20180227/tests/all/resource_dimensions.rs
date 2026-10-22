// SPDX-License-Identifier: CC0-1.0

//! `ListAvailableResourceDimensions`, end to end over the wire.
//!
//! RULE under test: for every requested metric type, the operation reports the
//! dimension groups the service publishes for the request's `ServiceType`, each
//! with the dimensions that group holds — a CAPABILITY catalog, identical for
//! every data source the account may read, paged by `MaxResults` over the
//! (metric type, group) records and persisting nothing.
//!
//! The catalog the rows below expect is this service's own model documentation,
//! stated independently of the handler's copy of it:
//!
//! ```text
//! smithy_query(query="doc",shape="DimensionGroup",member="Group")
//! smithy_query(query="doc",shape="DimensionGroup",member="Dimensions")
//! ```
//!
//! The recorded unit (`db.load` on `RDS`) is ONE row; the rows vary the metric
//! type, the metric count and order, the service type, the data source, the page
//! size and the `AuthorizedActions` set.

use serde_json::{json, Value};

use crate::pi_harness::{call, emulator, is_modelled_next_token, Answer};

/// Which `ServiceType` values the model documents a group for: `ALL` for a group
/// whose documentation names Amazon DocumentDB among other engines or restricts
/// nothing, `RDS` for "all engines except Amazon DocumentDB" (or a named
/// Aurora/RDS engine), `DOCDB` for "only Amazon DocumentDB".
const ALL: &str = "ALL";
const RDS_ONLY: &str = "RDS";
const DOCDB_ONLY: &str = "DOCDB";

/// The published catalog, as the model documents it: `(group, availability,
/// dimensions)`.
///
/// `db.plans` is absent because the documented `Dimensions` list gives it no
/// dimension; `db.session_type` is RDS from its group entry ("only Aurora
/// PostgreSQL and RDS PostgreSQL").
#[allow(clippy::type_complexity)]
const DOCUMENTED_CATALOG: &[(&str, &str, &[&str])] = &[
    ("db", ALL, &["db.name"]),
    ("db.application", ALL, &["db.application.name"]),
    (
        "db.blocking_object",
        ALL,
        &[
            "db.blocking_object.id",
            "db.blocking_object.type",
            "db.blocking_object.value",
        ],
    ),
    ("db.blocking_session", ALL, &["db.blocking_session.id"]),
    (
        "db.blocking_sql",
        ALL,
        &["db.blocking_sql.id", "db.blocking_sql.sql"],
    ),
    ("db.host", ALL, &["db.host.id", "db.host.name"]),
    (
        "db.query",
        DOCDB_ONLY,
        &[
            "db.query.db_id",
            "db.query.id",
            "db.query.statement",
            "db.query.tokenized_id",
        ],
    ),
    (
        "db.query_tokenized",
        DOCDB_ONLY,
        &[
            "db.query.tokenized.db_id",
            "db.query.tokenized.id",
            "db.query.tokenized.statement",
        ],
    ),
    ("db.session_type", RDS_ONLY, &["db.session_type.name"]),
    (
        "db.sql",
        RDS_ONLY,
        &[
            "db.sql.db_id",
            "db.sql.id",
            "db.sql.statement",
            "db.sql.tokenized_id",
        ],
    ),
    (
        "db.sql_tokenized",
        RDS_ONLY,
        &[
            "db.sql_tokenized.db_id",
            "db.sql_tokenized.id",
            "db.sql_tokenized.statement",
        ],
    ),
    ("db.user", RDS_ONLY, &["db.user.id", "db.user.name"]),
    (
        "db.wait_event",
        RDS_ONLY,
        &["db.wait_event.name", "db.wait_event.type"],
    ),
    (
        "db.wait_event_type",
        RDS_ONLY,
        &["db.wait_event_type.name"],
    ),
    ("db.wait_state", DOCDB_ONLY, &["db.wait_state.name"]),
];

/// The metric types the service publishes dimensions for: the DB load metric in
/// both of the forms `DescribeDimensionKeys`' `Metric` member documents it
/// (`db.load.avg`, the scaled active-session count, and `db.sampledload.avg`, the
/// raw one).
const DIMENSIONED_METRICS: &[&str] = &["db.load", "db.sampledload"];

/// The three values of the modelled `FineGrainedAction` enum.
const FINE_GRAINED_ACTIONS: &[&str] = &[
    "DescribeDimensionKeys",
    "GetDimensionKeyDetails",
    "GetResourceMetrics",
];

/// Data sources the authorization gate serves: an immutable DB resource id, in
/// the forms the `Identifier` members document and this emulator's database
/// services mint.
const MONITORED: &[&str] = &[
    "db-ABCDEFGHIJKLMNOPQRSTU1VW2X",
    "db-0123456789",
    "cluster-ABCDEFGHIJKL",
];

/// The groups the catalog publishes for one `ServiceType`, in the order a
/// response is expected to carry them (lexicographic by group name, which is the
/// order the table above is written in).
fn expected_groups(service_type: &str) -> Vec<(String, Vec<String>)> {
    DOCUMENTED_CATALOG
        .iter()
        .filter(|(_, availability, _)| *availability == ALL || *availability == service_type)
        .map(|(group, _, dimensions)| {
            (
                (*group).to_string(),
                dimensions.iter().map(|d| (*d).to_string()).collect(),
            )
        })
        .collect()
}

fn request(service_type: &str, identifier: &str, metrics: &[&str]) -> Value {
    json!({
        "ServiceType": service_type,
        "Identifier": identifier,
        "Metrics": metrics,
    })
}

/// The `MetricDimensions` entries of an answer, as `(metric, groups)` with each
/// group's dimensions.
#[allow(clippy::type_complexity)]
fn entries(answer: &Answer) -> Vec<(String, Vec<(String, Vec<String>)>)> {
    answer
        .member("MetricDimensions")
        .as_array()
        .map(|list| {
            list.iter()
                .map(|entry| {
                    let metric = entry
                        .get("Metric")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string();
                    let groups = entry
                        .get("Groups")
                        .and_then(Value::as_array)
                        .map(|groups| {
                            groups
                                .iter()
                                .map(|group| {
                                    let name = group
                                        .get("Group")
                                        .and_then(Value::as_str)
                                        .unwrap_or_default()
                                        .to_string();
                                    let dimensions = group
                                        .get("Dimensions")
                                        .and_then(Value::as_array)
                                        .map(|dims| {
                                            dims.iter()
                                                .map(|dim| {
                                                    dim.get("Identifier")
                                                        .and_then(Value::as_str)
                                                        .unwrap_or_default()
                                                        .to_string()
                                                })
                                                .collect()
                                        })
                                        .unwrap_or_default();
                                    (name, dimensions)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    (metric, groups)
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The answer's records flattened to the `(metric, group)` sequence a page is
/// measured in.
fn records(answer: &Answer) -> Vec<(String, String, Vec<String>)> {
    entries(answer)
        .into_iter()
        .flat_map(|(metric, groups)| {
            groups
                .into_iter()
                .map(move |(group, dimensions)| (metric.clone(), group, dimensions))
        })
        .collect()
}

/// RULE: every requested metric type is answered with the groups the catalog
/// publishes for the request's `ServiceType`, each carrying that group's
/// documented dimensions.
#[tokio::test]
async fn the_published_catalog_is_reported_per_metric_type_and_service_type() {
    let emulator = emulator();

    // The rows differ along what the rule generalises over: the service type
    // (both modelled values), the metric type (EVERY published one, on its own
    // and together), the order the types are requested in, a repeated type, and
    // the data source.
    let mut metric_lists: Vec<Vec<&str>> =
        DIMENSIONED_METRICS.iter().map(|m| vec![*m]).collect();
    metric_lists.push(DIMENSIONED_METRICS.to_vec());
    metric_lists.push(DIMENSIONED_METRICS.iter().rev().copied().collect());
    metric_lists.push(vec![DIMENSIONED_METRICS[0], DIMENSIONED_METRICS[0]]);

    for service_type in ["RDS", "DOCDB"] {
        let expected = expected_groups(service_type);
        for metrics in &metric_lists {
            let metrics = metrics.as_slice();
            // One metric type per entry: a repeated type is answered once.
            let mut expected_metrics: Vec<&str> = Vec::new();
            for metric in metrics {
                if !expected_metrics.contains(metric) {
                    expected_metrics.push(metric);
                }
            }

            for identifier in MONITORED {
                let case = format!("{service_type} / {metrics:?} / {identifier}");
                let answer =
                    call(&emulator, "ListAvailableResourceDimensions", request(service_type, identifier, metrics))
                        .await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} {:?}",
                    answer.status, answer.body
                );
                assert!(
                    answer.member("NextToken").is_null(),
                    "case {case}: expected no NextToken on a complete answer, got {:?}",
                    answer.body
                );

                let answered = entries(&answer);
                let answered_metrics: Vec<&str> =
                    answered.iter().map(|(metric, _)| metric.as_str()).collect();
                assert_eq!(
                    answered_metrics, expected_metrics,
                    "case {case}: expected one entry per distinct requested metric type in request \
                     order, got {answered_metrics:?}"
                );

                for (metric, groups) in &answered {
                    assert_eq!(
                        groups, &expected,
                        "case {case} / {metric}: expected the documented catalog for \
                         {service_type}, got {groups:?}"
                    );
                    for (group, dimensions) in groups {
                        // `DimensionGroupDetail.Group` and `DimensionDetail.Identifier`
                        // are both `length 0..256, pattern \S`.
                        for value in std::iter::once(group).chain(dimensions.iter()) {
                            assert!(
                                !value.is_empty()
                                    && value.len() <= 256
                                    && !value.chars().any(char::is_whitespace),
                                "case {case}: expected a modelled group/dimension name, got \
                                 {value:?}"
                            );
                        }
                        assert!(
                            !dimensions.is_empty(),
                            "case {case} / {group}: expected a group to publish at least one \
                             dimension, got {dimensions:?}"
                        );
                    }
                }
            }
        }
    }

    // The two service types are genuinely different sets, so the filter above is
    // not vacuous: SQL groups are RDS-only and query groups DocumentDB-only.
    let rds: Vec<String> = expected_groups("RDS")
        .into_iter()
        .map(|(group, _)| group)
        .collect();
    let docdb: Vec<String> = expected_groups("DOCDB")
        .into_iter()
        .map(|(group, _)| group)
        .collect();
    for (case, group, in_rds) in [
        ("db.sql is RDS-only", "db.sql", true),
        ("db.user is RDS-only", "db.user", true),
        ("db.query is DocumentDB-only", "db.query", false),
        ("db.wait_state is DocumentDB-only", "db.wait_state", false),
    ] {
        assert_eq!(
            rds.iter().any(|g| g == group),
            in_rds,
            "case {case}: expected RDS membership {in_rds}, got {rds:?}"
        );
        assert_eq!(
            docdb.iter().any(|g| g == group),
            !in_rds,
            "case {case}: expected DOCDB membership {}, got {docdb:?}",
            !in_rds
        );
    }
}

/// RULE: `MaxResults` pages the (metric type, group) records — the pages
/// concatenate to exactly the unpaged answer, each record once, and the last page
/// carries no `NextToken`.
#[tokio::test]
async fn max_results_pages_the_records_exactly_once_each() {
    let emulator = emulator();

    let metric_lists: &[&[&str]] = &[&["db.load"], &["db.load", "db.sampledload"]];

    for service_type in ["RDS", "DOCDB"] {
        for metrics in metric_lists {
            let unpaged = call(
                &emulator,
                "ListAvailableResourceDimensions",
                request(service_type, MONITORED[0], metrics),
            )
            .await;
            let whole = records(&unpaged);
            assert!(
                !whole.is_empty(),
                "case {service_type} / {metrics:?}: expected a non-empty catalog, got {:?}",
                unpaged.body
            );

            // `MaxResults` is modelled `range 0..25`: both published bounds, a
            // size of one, and sizes around the record count.
            for page_size in [0, 1, 2, 3, 7, 11, 24, 25] {
                let case = format!("{service_type} / {metrics:?} / MaxResults={page_size}");
                let mut collected: Vec<(String, String, Vec<String>)> = Vec::new();
                let mut token: Option<String> = None;
                let mut pages = 0;

                loop {
                    let mut input = request(service_type, MONITORED[0], metrics);
                    input["MaxResults"] = json!(page_size);
                    if let Some(token) = &token {
                        input["NextToken"] = json!(token);
                    }
                    let answer =
                        call(&emulator, "ListAvailableResourceDimensions", input).await;
                    assert_eq!(
                        answer.status, 200,
                        "case {case}: expected 200 on page {pages}, got {} {:?}",
                        answer.status, answer.body
                    );

                    let page = records(&answer);
                    if page_size > 0 {
                        assert!(
                            page.len() <= page_size,
                            "case {case}: expected at most {page_size} records on page {pages}, \
                             got {}",
                            page.len()
                        );
                    }
                    collected.extend(page);
                    pages += 1;

                    token = answer
                        .member("NextToken")
                        .as_str()
                        .map(|token| token.to_string());
                    match &token {
                        Some(token) => assert!(
                            is_modelled_next_token(token),
                            "case {case}: expected a modelled NextToken, got {token:?}"
                        ),
                        None => break,
                    }
                    assert!(
                        pages <= whole.len() + 1,
                        "case {case}: paging did not terminate after {pages} pages"
                    );
                }

                assert_eq!(
                    collected, whole,
                    "case {case}: expected the pages to concatenate to the unpaged answer"
                );
                let expected_pages = if page_size == 0 {
                    1
                } else {
                    whole.len().div_ceil(page_size)
                };
                assert_eq!(
                    pages, expected_pages,
                    "case {case}: expected {expected_pages} page(s) for {} records, got {pages}",
                    whole.len()
                );
            }
        }
    }

    // One past the published upper bound is refused at the boundary, by the
    // validation stage rather than by the handler.
    let mut too_many = request("RDS", MONITORED[0], &["db.load"]);
    too_many["MaxResults"] = json!(26);
    let refused = call(&emulator, "ListAvailableResourceDimensions", too_many).await;
    assert_eq!(
        refused.status, 400,
        "case MaxResults=26: expected 400, got {} {:?}",
        refused.status, refused.body
    );
    assert_eq!(
        refused.error_code(),
        "ValidationException",
        "case MaxResults=26: expected the validation stage's refusal, got {:?}",
        refused.body
    );

    // A cursor is bound to the result set it was handed out for: one issued for
    // a different metric type set is not honoured under another.
    let mut first = request("RDS", MONITORED[0], &["db.load"]);
    first["MaxResults"] = json!(1);
    let first = call(&emulator, "ListAvailableResourceDimensions", first).await;
    let token = first
        .member("NextToken")
        .as_str()
        .unwrap_or_else(|| panic!("a paged answer carries a cursor, got {:?}", first.body))
        .to_string();
    for (case, service_type, identifier, metrics) in [
        ("another metric type", "RDS", MONITORED[0], &["db.sampledload"][..]),
        ("another service type", "DOCDB", MONITORED[0], &["db.load"][..]),
        ("another data source", "RDS", MONITORED[1], &["db.load"][..]),
    ] {
        let mut input = request(service_type, identifier, metrics);
        input["MaxResults"] = json!(1);
        input["NextToken"] = json!(token);
        let answer = call(&emulator, "ListAvailableResourceDimensions", input).await;
        assert_eq!(
            answer.error_code(),
            "InvalidArgumentException",
            "case a cursor reused for {case}: expected InvalidArgumentException, got {} {:?}",
            answer.status,
            answer.body
        );
    }
}

/// RULE: a metric type the service publishes no dimensions for is refused with
/// the modelled `InvalidArgumentException`, and a data source this account does
/// not monitor with the modelled `NotAuthorizedException` — which is decided
/// first, so the catalog never discloses anything about an unmonitored resource.
#[tokio::test]
async fn an_unpublished_metric_type_and_an_unmonitored_data_source_are_refused() {
    let emulator = emulator();

    // Each row is inside the modelled `DimensionsMetricList` member constraints
    // (length 1..5, each element `^[a-zA-Z0-9-_\.:/*)( ]+$`), so the handler is
    // what answers: metric types of OTHER Performance Insights metric families
    // (`ListAvailableResourceMetrics`' `MetricTypes` documents os, db,
    // db.sql.stats, db.sql_tokenized.stats), a full metric NAME rather than a
    // type, and a mixed list.
    let unpublished: &[&[&str]] = &[
        &["os"],
        &["db"],
        &["db.sql.stats"],
        &["db.sql_tokenized.stats"],
        &["db.load.avg"],
        &["db.sampledload.avg"],
        &["db.load", "os"],
        &["os", "db.load"],
    ];

    for metrics in unpublished {
        for service_type in ["RDS", "DOCDB"] {
            let case = format!("{service_type} / {metrics:?}");
            let answer = call(
                &emulator,
                "ListAvailableResourceDimensions",
                request(service_type, MONITORED[0], metrics),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgumentException",
                "case {case}: expected InvalidArgumentException, got {:?}",
                answer.body
            );
            assert!(
                answer.member("MetricDimensions").is_null(),
                "case {case}: expected no catalog in a refusal, got {:?}",
                answer.body
            );
        }
    }

    // Identifiers that address no data source this account monitors: a DB
    // instance NAME, the identifier the wire corpus probes with, a lowercase
    // token, and a kind prefix with an empty token.
    let unmonitored = [
        "my-production-database",
        "sim2real-probe-pi-dbinst8-e56fa096",
        "db-abcdefghijkl",
        "db-",
    ];
    for identifier in unmonitored {
        for metrics in [&["db.load"][..], &["os"][..]] {
            let case = format!("{identifier} / {metrics:?}");
            let answer = call(
                &emulator,
                "ListAvailableResourceDimensions",
                request("RDS", identifier, metrics),
            )
            .await;
            assert_eq!(
                answer.status, 400,
                "case {case}: expected 400, got {} {:?}",
                answer.status, answer.body
            );
            // The gate answers BEFORE the metric types are judged, so an
            // unmonitored data source is the authorization refusal whether or
            // not the request's metric types are published ones.
            assert_eq!(
                answer.error_code(),
                "NotAuthorizedException",
                "case {case}: expected NotAuthorizedException, got {:?}",
                answer.body
            );
            assert_eq!(
                answer.error_message(),
                service_com_amazonaws_pi_performanceinsightsv20180227::state::NOT_AUTHORIZED_MESSAGE,
                "case {case}: expected the service's uniform authorization message, got {:?}",
                answer.body
            );
        }
    }
}

/// RULE: `AuthorizedActions` narrows the answer to the dimensions the caller may
/// read through every named action; this emulator evaluates no IAM policy, so
/// every Performance Insights action is authorized and each subset answers the
/// same published catalog — the documented behaviour for a request naming none.
#[tokio::test]
async fn every_authorized_actions_subset_answers_the_published_catalog() {
    let emulator = emulator();

    for service_type in ["RDS", "DOCDB"] {
        let baseline = call(
            &emulator,
            "ListAvailableResourceDimensions",
            request(service_type, MONITORED[0], &["db.load"]),
        )
        .await;
        assert_eq!(
            baseline.status, 200,
            "case {service_type} / no AuthorizedActions: expected 200, got {} {:?}",
            baseline.status, baseline.body
        );

        // Every subset of the modelled `FineGrainedAction` enum the member's
        // `length 0..3` bound admits: the empty list, each single action, each
        // pair, and all three.
        let mut subsets: Vec<Vec<&str>> = vec![Vec::new()];
        for (index, first) in FINE_GRAINED_ACTIONS.iter().enumerate() {
            subsets.push(vec![*first]);
            for second in FINE_GRAINED_ACTIONS.iter().skip(index + 1) {
                subsets.push(vec![*first, *second]);
            }
        }
        subsets.push(FINE_GRAINED_ACTIONS.to_vec());

        for subset in subsets {
            let case = format!("{service_type} / AuthorizedActions={subset:?}");
            let mut input = request(service_type, MONITORED[0], &["db.load"]);
            input["AuthorizedActions"] = json!(subset);
            let answer = call(&emulator, "ListAvailableResourceDimensions", input).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {} {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                entries(&answer),
                entries(&baseline),
                "case {case}: expected the same published catalog as a request naming no action"
            );
        }
    }

    // An action outside the modelled enum is refused at the boundary.
    let mut input = request("RDS", MONITORED[0], &["db.load"]);
    input["AuthorizedActions"] = json!(["ListAvailableResourceDimensions"]);
    let answer = call(&emulator, "ListAvailableResourceDimensions", input).await;
    assert_eq!(
        answer.status, 400,
        "case an unmodelled FineGrainedAction: expected 400, got {} {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code(),
        "ValidationException",
        "case an unmodelled FineGrainedAction: expected the validation stage's refusal, got {:?}",
        answer.body
    );
}

/// RULE: listing the available dimensions is a READ — it answers identically
/// however often it is called, and changes nothing a later caller sees.
#[tokio::test]
async fn listing_the_available_dimensions_persists_nothing() {
    let emulator = emulator();

    let identifier = MONITORED[0];
    let first = call(
        &emulator,
        "ListAvailableResourceDimensions",
        request("RDS", identifier, &["db.load", "db.sampledload"]),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case the first call: expected 200, got {} {:?}",
        first.status, first.body
    );

    // A paged walk in between, so any cursor bookkeeping would have had its
    // chance to leave a trace.
    let mut paged = request("RDS", identifier, &["db.load"]);
    paged["MaxResults"] = json!(2);
    let paged = call(&emulator, "ListAvailableResourceDimensions", paged).await;
    assert_eq!(
        paged.status, 200,
        "case the paged call: expected 200, got {} {:?}",
        paged.status, paged.body
    );

    let second = call(
        &emulator,
        "ListAvailableResourceDimensions",
        request("RDS", identifier, &["db.load", "db.sampledload"]),
    )
    .await;
    assert_eq!(
        second.body, first.body,
        "case the repeated call: expected an identical answer, got {:?}",
        second.body
    );

    // And the data source holds no analysis report because dimensions were
    // listed for it.
    let reports = call(
        &emulator,
        "ListPerformanceAnalysisReports",
        json!({"ServiceType": "RDS", "Identifier": identifier}),
    )
    .await;
    assert_eq!(
        reports.member("AnalysisReports").as_array(),
        Some(&Vec::new()),
        "case the reports of a listed data source: expected none, got {:?}",
        reports.body
    );
}
