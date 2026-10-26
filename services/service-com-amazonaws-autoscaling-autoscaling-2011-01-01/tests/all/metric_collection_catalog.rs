// SPDX-License-Identifier: CC0-1.0

//! The group-metric catalog `DescribeMetricCollectionTypes` publishes, over the
//! awsQuery wire.
//!
//! THE RULE UNDER TEST: a reference-data describe publishes the service's whole
//! current set of valid values, which is what a client enumerates to decide what
//! it may send — so where the on-disk model's prose for the member enumerates
//! fewer values than the service accepts, that prose is a lower bound and the
//! published catalog is the complete set, with nothing in it the service does not
//! accept.
//!
//! The table below is the class, not one row of it: it spans every metric the
//! catalog publishes — the sizes, the instance counts, the capacities, the
//! retained-capacity series and the warm pool, each row recording whether the
//! model member's own documentation enumerates it — and a second table of
//! near-miss names that are NOT metrics. The catalog itself is hand-written data
//! in `src/state.rs`, whose doc comment names its source: the model member
//! `MetricCollectionType$Metric` (read with
//! `smithy_query(query="shape",shape="com.amazonaws.autoscaling#MetricCollectionType")`)
//! for the metrics it enumerates, plus the warm pool minimum and the
//! retained-capacity series that member's documentation omits.
//!
//! Every assertion drives the service the way the router does — one
//! `MultiServiceRouter` holding this service over one shared engine — so what is
//! under test is the HTTP behaviour, not a handler called directly. Stability
//! across reads and across what the store holds belongs to
//! `reference_data_catalogs.rs`, which walks every catalog through it.

use crate::support::{new_engine, query, router_with, tag_values};

use service_com_amazonaws_autoscaling_autoscaling_2011_01_01 as autoscaling;
use autoscaling::state;

/// One metric the catalog must publish.
struct MetricCase {
    /// The metric's name, as `EnableMetricsCollection` names it.
    metric: &'static str,
    /// What the metric reports on, so a row names its part of the class.
    family: &'static str,
    /// Whether the model member's own documentation enumerates this metric.
    /// `false` marks a metric the service publishes and that documentation is
    /// behind on.
    in_model_prose: bool,
}

/// Every group metric the service accepts, one row each.
const METRICS: &[MetricCase] = &[
    MetricCase { metric: "GroupMinSize", family: "group size", in_model_prose: true },
    MetricCase { metric: "GroupMaxSize", family: "group size", in_model_prose: true },
    MetricCase { metric: "GroupDesiredCapacity", family: "group size", in_model_prose: true },
    MetricCase { metric: "GroupInServiceInstances", family: "group instances", in_model_prose: true },
    MetricCase { metric: "GroupPendingInstances", family: "group instances", in_model_prose: true },
    MetricCase { metric: "GroupStandbyInstances", family: "group instances", in_model_prose: true },
    MetricCase { metric: "GroupTerminatingInstances", family: "group instances", in_model_prose: true },
    MetricCase {
        metric: "GroupTerminatingRetainedInstances",
        family: "group instances (retained)",
        in_model_prose: false,
    },
    MetricCase { metric: "GroupTotalInstances", family: "group instances", in_model_prose: true },
    MetricCase { metric: "GroupInServiceCapacity", family: "group capacity", in_model_prose: true },
    MetricCase { metric: "GroupPendingCapacity", family: "group capacity", in_model_prose: true },
    MetricCase { metric: "GroupStandbyCapacity", family: "group capacity", in_model_prose: true },
    MetricCase { metric: "GroupTerminatingCapacity", family: "group capacity", in_model_prose: true },
    MetricCase {
        metric: "GroupTerminatingRetainedCapacity",
        family: "group capacity (retained)",
        in_model_prose: false,
    },
    MetricCase { metric: "GroupTotalCapacity", family: "group capacity", in_model_prose: true },
    MetricCase { metric: "WarmPoolMinSize", family: "warm pool size", in_model_prose: false },
    MetricCase { metric: "WarmPoolDesiredCapacity", family: "warm pool capacity", in_model_prose: true },
    MetricCase { metric: "WarmPoolWarmedCapacity", family: "warm pool capacity", in_model_prose: true },
    MetricCase { metric: "WarmPoolPendingCapacity", family: "warm pool capacity", in_model_prose: true },
    MetricCase {
        metric: "WarmPoolPendingRetainedCapacity",
        family: "warm pool capacity (retained)",
        in_model_prose: false,
    },
    MetricCase {
        metric: "WarmPoolTerminatingCapacity",
        family: "warm pool capacity",
        in_model_prose: true,
    },
    MetricCase {
        metric: "WarmPoolTerminatingRetainedCapacity",
        family: "warm pool capacity (retained)",
        in_model_prose: false,
    },
    MetricCase { metric: "WarmPoolTotalCapacity", family: "warm pool capacity", in_model_prose: true },
    MetricCase {
        metric: "GroupAndWarmPoolDesiredCapacity",
        family: "group and warm pool",
        in_model_prose: true,
    },
    MetricCase {
        metric: "GroupAndWarmPoolTotalCapacity",
        family: "group and warm pool",
        in_model_prose: true,
    },
];

/// Names a client might plausibly send that are NOT group metrics: the catalog
/// publishes the complete set and nothing beyond it, so each of these must be
/// absent from the answer and rejected by the accepted-metric check.
///
/// One per way of being wrong: a family that has no such series, a bound that
/// exists for the group but not for the warm pool, a case difference, a singular
/// where the catalog is plural, a retained form of a metric that has none, and a
/// name padded with whitespace.
const NOT_METRICS: &[&str] = &[
    "GroupRetainedInstances",
    "WarmPoolMaxSize",
    "groupminsize",
    "GroupTerminatingRetainedInstance",
    "GroupMinSizeRetained",
    " WarmPoolMinSize",
];

/// The whole catalog, read off the wire: every metric of [`METRICS`] published
/// exactly once, in the order the service holds it, with no entry the service
/// does not accept.
#[tokio::test]
async fn the_published_catalog_is_every_group_metric_the_service_accepts() {
    let router = router_with(new_engine());

    let (status, body) = query(&router, "DescribeMetricCollectionTypes", &[]).await;
    assert_eq!(
        status, 200,
        "DescribeMetricCollectionTypes: a Unit-input read models no 4xx, got {status}: {body}"
    );
    let published = tag_values(&body, "Metric");

    for case in METRICS {
        let occurrences = published.iter().filter(|entry| *entry == case.metric).count();
        assert_eq!(
            occurrences, 1,
            "case {:?} ({}, in model prose: {}): expected the catalog to publish it exactly \
             once, got {occurrences} occurrences in {published:?}",
            case.metric, case.family, case.in_model_prose
        );
    }

    // The rows the model member's documentation omits are published all the same:
    // the prose is a lower bound, the catalog is the set.
    let omitted_by_prose: Vec<&str> = METRICS
        .iter()
        .filter(|case| !case.in_model_prose)
        .map(|case| case.metric)
        .collect();
    assert!(
        !omitted_by_prose.is_empty(),
        "the table must still span the metrics the model member's prose omits, \
         got {omitted_by_prose:?}"
    );
    for metric in &omitted_by_prose {
        assert!(
            published.iter().any(|entry| entry == metric),
            "case {metric:?}: a metric the service accepts must be published even where the \
             model member's documentation omits it, got {published:?}"
        );
    }

    for metric in NOT_METRICS {
        assert!(
            !published.iter().any(|entry| entry == metric),
            "case {metric:?}: the catalog publishes only metrics the service accepts, \
             got {published:?}"
        );
    }

    // No entry beyond the table: the published set and the table are the same set.
    for entry in &published {
        assert!(
            METRICS.iter().any(|case| case.metric == entry),
            "case {entry:?}: published but not a group metric this suite knows, \
             got {published:?}"
        );
    }
    assert_eq!(
        published.len(),
        METRICS.len(),
        "expected the catalog to publish {} metrics, got {published:?}",
        METRICS.len()
    );

    // The second published list: the one granularity metrics collection accepts.
    let granularities = tag_values(&body, "Granularity");
    assert_eq!(
        granularities,
        vec!["1Minute".to_string()],
        "expected the single published granularity, got {granularities:?}"
    );
}

/// The published catalog and the metrics the mutating operations accept are ONE
/// list: the accepted-metric check answers true for every name the catalog
/// publishes and false for every near miss, so neither side can drift from the
/// other.
#[tokio::test]
async fn the_accepted_metric_check_is_the_published_catalog() {
    let router = router_with(new_engine());

    let (status, body) = query(&router, "DescribeMetricCollectionTypes", &[]).await;
    assert_eq!(status, 200, "DescribeMetricCollectionTypes: got {status}: {body}");
    let published = tag_values(&body, "Metric");

    // The positive side is driven from this suite's own table, not from the
    // answer: a name the predicate and the response both read off one constant
    // would agree however wrong that constant was.
    for case in METRICS {
        assert!(
            state::is_valid_metric_collection_type(case.metric),
            "case {:?} ({}): a group metric must be accepted, got false",
            case.metric,
            case.family
        );
        assert!(
            published.iter().any(|entry| entry == case.metric),
            "case {:?} ({}): an accepted metric must be published, got {published:?}",
            case.metric,
            case.family
        );
    }
    for metric in NOT_METRICS {
        assert!(
            !state::is_valid_metric_collection_type(metric),
            "case {metric:?}: a name the catalog does not publish must not be accepted, got true"
        );
    }
}

// Stability across repeated reads and across what the store holds is NOT asserted
// again here: `reference_data_catalogs.rs` already reads every catalog, this one
// among them, before and after the store gains and loses a group.
