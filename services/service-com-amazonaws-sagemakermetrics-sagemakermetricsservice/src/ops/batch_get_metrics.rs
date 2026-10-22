// SPDX-License-Identifier: CC0-1.0

//! @op BatchGetMetrics
//! Retrieves training metrics, computed from what `BatchPutMetrics` stored.
//!
//! Each `MetricQuery` is answered on its own and positionally: result `i` answers query
//! `i`, and one query's failure never fails its siblings.
//!
//! One query is evaluated in four steps:
//!
//! 1. `ResourceArn` resolves to a trial component: the name is the ARN's final
//!    `/`-separated segment, matched against the `TrialComponentName` the points were
//!    written under.
//! 2. The `MetricName` series is filtered by `Start`/`End` (inclusive) on the axis
//!    `XAxisType` names — the point's epoch second for `Timestamp`, its `Step` for
//!    `IterationNumber`.
//! 3. The surviving points are bucketed by `Period`: a fixed-width window on the
//!    timestamp axis for `OneMinute`/`FiveMinute`/`OneHour` (keyed by the bucket's first
//!    epoch second), or one bucket per `Step` for `IterationNumber`.
//! 4. Each bucket is reduced by `MetricStat` to one `MetricValues` entry, beside its
//!    bucket key in `XAxisValues`; both lists are the same length and ascending in x.
//!
//! `Status` reports what happened to that one query:
//!
//! * `Complete` — every matching bucket is in the result. An unknown resource or metric
//!   name is `Complete` with empty lists: nothing was stored, which is not a failure.
//! * `Truncated` — more than [`MAX_RESULT_POINTS_PER_QUERY`] buckets matched; the lowest
//!   that many in x are returned.
//! * `ValidationError` — the query cannot be evaluated as asked (an ARN with no trailing
//!   name segment, `Start` after `End`, an enum value outside the model), with a
//!   `Message` naming the member.
//! * `InternalError` — the shared store is unreachable.
//!
//! This operation only READS state: polling it changes nothing a later caller sees.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `BatchGetMetrics`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-sagemakermetrics-sagemakermetricsservice/smithy-model/com-amazonaws-sagemakermetrics-sagemakermetricsservice.json")
//! ```
//!
//! Used to retrieve training metrics from SageMaker.
//!
//! ## Input
//!
//! * `MetricQueries` (MetricQueryList, len 1..100) **required** — Queries made to retrieve
//!   training metrics from SageMaker.
//!
//! ## Output
//!
//! * `MetricQueryResults` (MetricQueryResultList, len 1..100) — The results of a query to
//!   retrieve training metrics from SageMaker.

#[allow(unused_imports)]
use super::types::*;

use crate::state::{ComAmazonawsSagemakermetricsSagemakermetricsserviceState, MetricDatum};
use std::collections::BTreeMap;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `BatchGetMetricsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakermetrics-sagemakermetricsservice.json");

/// How many aggregated points one query may return before the rest are dropped and the
/// result is reported `Truncated`.
///
/// AWS publishes no number for this, so the cap is chosen by this emulator; it exists
/// because the modelled `MetricQueryResultStatus::Truncated` has to be reachable at all.
pub const MAX_RESULT_POINTS_PER_QUERY: usize = 1_000;

/// `MetricQueryResultStatus` members, as `types::METRIC_QUERY_RESULT_STATUS_VALUES`
/// lists them.
const STATUS_COMPLETE: &str = "Complete";
/// See [`STATUS_COMPLETE`].
const STATUS_INTERNAL_ERROR: &str = "InternalError";
/// See [`STATUS_COMPLETE`].
const STATUS_VALIDATION_ERROR: &str = "ValidationError";

/// See [`STATUS_COMPLETE`].
const STATUS_TRUNCATED: &str = "Truncated";

/// How much of a request value a `Message` may quote back.
///
/// The modelled `Message` is `length 0..2048`, so an echoed value is clipped well inside
/// that bound: a caller cannot push the response member past its own constraint.
const MAX_QUOTED_VALUE_CHARS: usize = 64;

/// `value`, clipped to [`MAX_QUOTED_VALUE_CHARS`] characters on a character boundary.
fn quoted(value: &str) -> std::borrow::Cow<'_, str> {
    match value.char_indices().nth(MAX_QUOTED_VALUE_CHARS) {
        None => std::borrow::Cow::Borrowed(value),
        Some((end, _)) => std::borrow::Cow::Owned(format!("{}…", &value[..end])),
    }
}

/// The `MetricStatistic` a query reduces each bucket with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Statistic {
    Min,
    Max,
    Avg,
    Count,
    StdDev,
    Last,
}

impl Statistic {
    /// Parse a wire `MetricStatistic` value; `None` for anything the model does not list
    /// (`types::METRIC_STATISTIC_VALUES`).
    fn parse(wire: &str) -> Option<Self> {
        match wire {
            "Min" => Some(Self::Min),
            "Max" => Some(Self::Max),
            "Avg" => Some(Self::Avg),
            "Count" => Some(Self::Count),
            "StdDev" => Some(Self::StdDev),
            "Last" => Some(Self::Last),
            _ => None,
        }
    }

    /// Reduce one bucket's values, which arrive ascending by `(timestamp, step)`, to the
    /// single number this statistic names. `values` is never empty: a bucket exists only
    /// because a point fell in it. Every stored value is finite (`BatchPutMetrics`
    /// refuses anything else), so every reduction below is finite too and
    /// `MetricValues` only ever carries JSON-representable numbers.
    fn reduce(self, values: &[f64]) -> f64 {
        let count = values.len() as f64;
        let sum: f64 = values.iter().sum();
        match self {
            Self::Min => values.iter().copied().fold(f64::INFINITY, f64::min),
            Self::Max => values.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            Self::Avg => sum / count,
            Self::Count => count,
            Self::StdDev => {
                // Population standard deviation over the bucket's own points: the
                // bucket is the whole population being summarised, so a one-point
                // bucket has a spread of zero rather than an undefined one.
                let mean = sum / count;
                let variance =
                    values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / count;
                variance.sqrt()
            }
            // The series is kept sorted, so the bucket's last value is its latest. The
            // fallback is unreachable (a bucket holds at least one point).
            Self::Last => values.last().copied().unwrap_or(f64::NAN),
        }
    }
}

/// How a query groups points: the `Period`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Grouping {
    /// A fixed-width window on the timestamp axis, in seconds.
    Seconds(i64),
    /// One bucket per `Step`.
    PerIteration,
}

impl Grouping {
    /// Parse a wire `Period` value; `None` for anything the model does not list
    /// (`types::PERIOD_VALUES`).
    fn parse(wire: &str) -> Option<Self> {
        match wire {
            "OneMinute" => Some(Self::Seconds(60)),
            "FiveMinute" => Some(Self::Seconds(5 * 60)),
            "OneHour" => Some(Self::Seconds(60 * 60)),
            "IterationNumber" => Some(Self::PerIteration),
            _ => None,
        }
    }

    /// The bucket `datum` falls in: the first epoch second of its window, or its `Step`.
    /// `None` when the point has no position for this grouping — a point written without
    /// a `Step` has no iteration number, and an instant so extreme that its window start
    /// overflows `i64` seconds has no window.
    fn bucket_key(self, datum: &MetricDatum) -> Option<i64> {
        match self {
            Self::Seconds(width) => datum.epoch_seconds().div_euclid(width).checked_mul(width),
            Self::PerIteration => datum.step.map(i64::from),
        }
    }
}

/// The axis `Start`/`End` bound: the `XAxisType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Axis {
    Timestamp,
    IterationNumber,
}

impl Axis {
    /// Parse a wire `XAxisType` value; `None` for anything the model does not list
    /// (`types::X_AXIS_TYPE_VALUES`).
    fn parse(wire: &str) -> Option<Self> {
        match wire {
            "Timestamp" => Some(Self::Timestamp),
            "IterationNumber" => Some(Self::IterationNumber),
            _ => None,
        }
    }

    /// Where `datum` sits on this axis: its epoch second, or its `Step`. `None` when the
    /// point has no position on the axis, which excludes it from a filtered query.
    fn position(self, datum: &MetricDatum) -> Option<i64> {
        match self {
            Self::Timestamp => Some(datum.epoch_seconds()),
            Self::IterationNumber => datum.step.map(i64::from),
        }
    }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `BatchGetMetricsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.sagemakermetrics#BatchGetMetrics")]
#[derive(Debug)]
pub enum BatchGetMetricsErrorModeled {
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::batch_get_metrics::BatchGetMetricsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type BatchGetMetricsError = crate::scaffolded::Scaffolded<BatchGetMetricsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<BatchGetMetricsErrorModeled> for BatchGetMetricsError {
    fn from(modeled: BatchGetMetricsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `BatchGetMetrics` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct BatchGetMetrics<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> BatchGetMetrics<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// A result that carries only a status and a message: the two failing statuses.
fn failed(status: &str, message: String) -> MetricQueryResult {
    MetricQueryResult {
        status: status.to_string(),
        message: Some(message),
        x_axis_values: Vec::new(),
        metric_values: Vec::new(),
    }
}

/// The trial component a `ResourceArn` names: the final `/`-separated segment of the
/// ARN. `None` when there is no such segment, so the ARN names no resource — which the
/// modelled `@pattern` allows, since it only requires the resource part to CONTAIN a `/`.
fn trial_component_of(resource_arn: &str) -> Option<&str> {
    let (_, name) = resource_arn.rsplit_once('/')?;
    (!name.is_empty()).then_some(name)
}

/// Answer one query from the store. Pure: it reads state and writes none.
fn evaluate(
    state: &ComAmazonawsSagemakermetricsSagemakermetricsserviceState,
    query: &MetricQuery,
    index: usize,
) -> MetricQueryResult {
    let Some(statistic) = Statistic::parse(&query.metric_stat) else {
        return failed(
            STATUS_VALIDATION_ERROR,
            format!(
                "MetricQueries[{index}].MetricStat is not a MetricStatistic: {}",
                quoted(&query.metric_stat)
            ),
        );
    };
    let Some(grouping) = Grouping::parse(&query.period) else {
        return failed(
            STATUS_VALIDATION_ERROR,
            format!(
                "MetricQueries[{index}].Period is not a Period: {}",
                quoted(&query.period)
            ),
        );
    };
    let Some(axis) = Axis::parse(&query.x_axis_type) else {
        return failed(
            STATUS_VALIDATION_ERROR,
            format!(
                "MetricQueries[{index}].XAxisType is not an XAxisType: {}",
                quoted(&query.x_axis_type)
            ),
        );
    };
    let Some(trial_component) = trial_component_of(&query.resource_arn) else {
        return failed(
            STATUS_VALIDATION_ERROR,
            format!(
                "MetricQueries[{index}].ResourceArn names no SageMaker resource: \
                 the ARN has no trailing resource name segment"
            ),
        );
    };
    if let (Some(start), Some(end)) = (query.start, query.end) {
        if start > end {
            return failed(
                STATUS_VALIDATION_ERROR,
                format!(
                    "MetricQueries[{index}].Start ({start}) must not be greater than \
                     MetricQueries[{index}].End ({end})"
                ),
            );
        }
    }

    // Buckets ascending in x, each holding its points' values in (timestamp, step)
    // order, because the stored series is kept in that order.
    let mut buckets: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for datum in state.series(trial_component, &query.metric_name) {
        // A point outside the requested window, or with no position on the queried
        // axis, is not part of the answer.
        let position = axis.position(datum);
        if query.start.is_some() || query.end.is_some() {
            let Some(position) = position else { continue };
            if query.start.is_some_and(|start| position < start)
                || query.end.is_some_and(|end| position > end)
            {
                continue;
            }
        }
        let Some(key) = grouping.bucket_key(datum) else {
            continue;
        };
        buckets.entry(key).or_default().push(datum.value);
    }

    let truncated = buckets.len() > MAX_RESULT_POINTS_PER_QUERY;
    let returned = buckets.len().min(MAX_RESULT_POINTS_PER_QUERY);
    let mut x_axis_values = Vec::with_capacity(returned);
    let mut metric_values = Vec::with_capacity(returned);
    for (key, values) in buckets.iter().take(MAX_RESULT_POINTS_PER_QUERY) {
        x_axis_values.push(*key);
        metric_values.push(statistic.reduce(values));
    }

    MetricQueryResult {
        status: if truncated {
            STATUS_TRUNCATED.to_string()
        } else {
            STATUS_COMPLETE.to_string()
        },
        message: truncated.then(|| {
            format!(
                "MetricQueries[{index}] matched {} aggregated points; the lowest {MAX_RESULT_POINTS_PER_QUERY} in x are returned",
                buckets.len()
            )
        }),
        x_axis_values,
        metric_values,
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for BatchGetMetrics<Eng>
{
    type Input = BatchGetMetricsInput;
    type Output = BatchGetMetricsOutput;
    type Error = BatchGetMetricsError;

    const NAME: &'static str = "BatchGetMetrics";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_GET_METRICS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::BATCH_GET_METRICS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            // No shared store to read. The model has no operation-level error for
            // that, but every query can report it for itself.
            let results = (0..input.metric_queries.len())
                .map(|index| {
                    failed(
                        STATUS_INTERNAL_ERROR,
                        format!("MetricQueries[{index}] could not be read: the metric store is unavailable"),
                    )
                })
                .collect();
            return Ok(BatchGetMetricsOutput {
                metric_query_results: Some(results),
            });
        };

        let state = state.lock();
        let results: Vec<MetricQueryResult> = input
            .metric_queries
            .iter()
            .enumerate()
            .map(|(index, query)| evaluate(&state, query, index))
            .collect();
        drop(state);

        // An access record for a read of stored metric state: what was called, for how
        // many queries, and how many of them were not answered in full. Emitted at info
        // so the default filter keeps it. No metric value and nothing header-derived is
        // logged.
        tracing::info!(
            operation = Self::NAME,
            method = %ctx.parts().method,
            queries = results.len(),
            not_complete = results
                .iter()
                .filter(|result| result.status != STATUS_COMPLETE)
                .count(),
            "read training metrics",
        );

        Ok(BatchGetMetricsOutput {
            metric_query_results: Some(results),
        })
    }
}
