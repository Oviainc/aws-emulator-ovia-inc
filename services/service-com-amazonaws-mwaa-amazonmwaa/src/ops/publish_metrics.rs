// SPDX-License-Identifier: CC0-1.0

//! @op PublishMetrics
//!
//! Publishes environment health metrics for one environment: every datum of the request is
//! appended to the metric series it addresses on THAT environment's record
//! ([`crate::state::EnvironmentMetrics`]), so a datapoint one request published is part of
//! the series every later one reads.
//!
//! A metric is identified by its name TOGETHER with its dimension set, and that set is
//! unordered, so the series key holds the dimensions as a sorted map: two datums that name
//! the same dimensions in a different order land in one series, and the same metric name
//! under different dimensions lands in two. A datum states its observations either as one
//! `Value` or as a `StatisticValues` set; both are stored as the statistics of a set of
//! observations ([`crate::state::MetricStatistics`]), a single value being one observation
//! that is its own sum, minimum and maximum, so the aggregation of a series needs no case
//! for the form each datapoint arrived in.
//!
//! The model marks this operation internal, so nothing it accepts is reported by another
//! operation of this service: what it answers is the modelled empty output at HTTP 200.
//!
//! ## Which endpoint hosts it
//!
//! The model hosts this operation on the INTERNAL OPERATIONS endpoint (`@endpoint` host
//! prefix `ops.`), an endpoint class of its own: neither the control plane (`api.`) nor the
//! per-environment Apache Airflow front door (`env.`) — see
//! [`crate::service::endpoints`]. Nothing resolves the environment ahead of this handler, so
//! a name that resolves to nothing is refused HERE, in the only client error the model gives
//! this operation, and worded by the one builder every refusal this service's own layer
//! raises for a named environment uses ([`crate::state::environment_not_found_message`]).
//!
//! ## What is refused
//!
//! The modelled constraints — `EnvironmentName`'s length and pattern, `MetricName` and
//! `Timestamp` being required, `Unit`'s value set — are refused by the boundary validator
//! BEFORE this handler runs. What it refuses with the modelled `ValidationException`
//! (HTTP 400) are the rules the model cannot state, judged on the request alone and ahead of
//! the environment lookup, so a request carrying one datum the service cannot publish stores
//! none of them:
//!
//! * a datum that states both `Value` and `StatisticValues`, or neither;
//! * a number — a `Value` or any member of a `StatisticValues` set — that is not finite or is
//!   larger in magnitude than the bound [`crate::state::is_publishable_metric_value`] states;
//! * a `StatisticValues` set missing a member, counting no sample, or whose minimum exceeds
//!   its maximum: it summarises no set of observations;
//! * a blank or over-long `MetricName`, dimension name or dimension value, a datum carrying
//!   more than [`crate::state::MAX_METRIC_DIMENSIONS`] dimensions, or one naming a dimension
//!   twice (which would silently collapse into one);
//! * a `Timestamp` so far from the epoch that it is not expressible in epoch milliseconds;
//! * an `EnvironmentName` this Region holds no environment of, and an environment whose
//!   teardown has started, which takes no change ([`crate::state::EnvironmentRecord::change_refusal`]);
//! * a request that would take the environment past [`crate::state::MAX_METRIC_SERIES`]
//!   series.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back.
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `PublishMetrics`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-mwaa-amazonmwaa/smithy-model/com-amazonaws-mwaa-amazonmwaa.json")
//! ```
//!
//! Internal only. Publishes environment health metrics to Amazon CloudWatch.
//!
//! ## Input
//!
//! * `EnvironmentName` (EnvironmentName, len 1..80, pattern ^[a-zA-Z][0-9a-zA-Z-_]*$)
//!   **required** — Internal only. The name of the environment.
//! * `MetricData` (MetricData) **required** — Internal only. Publishes metrics to Amazon
//!   CloudWatch. To learn more about the metrics published to Amazon CloudWatch, see Amazon
//!   MWAA performance metrics in Amazon CloudWatch.
//!
//! ## Errors
//!
//! * `InternalServerException` (HTTP 500) — InternalServerException: An internal error has
//!   occurred.
//! * `ValidationException` (HTTP 400) — ValidationException: The provided input is not valid.

#[allow(unused_imports)]
use super::types::*;

use std::collections::BTreeMap;

use crate::state::{
    is_publishable_metric_value, MetricDatapoint, MetricRejection, MetricStatistics,
    PublishedMetric,
};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `PublishMetricsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-mwaa-amazonmwaa.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `PublishMetricsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.mwaa#PublishMetrics")]
#[derive(Debug)]
pub enum PublishMetricsErrorModeled {
    InternalServerException(InternalServerException),
    ValidationException(ValidationException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::publish_metrics::PublishMetricsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type PublishMetricsError = crate::scaffolded::Scaffolded<PublishMetricsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<PublishMetricsErrorModeled> for PublishMetricsError {
    fn from(modeled: PublishMetricsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// This operation's refusal of a request the service will not publish: the modelled
/// `ValidationException` (HTTP 400), carrying the sentence that says which member was wrong
/// and why.
fn validation(message: impl Into<String>) -> PublishMetricsError {
    PublishMetricsErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
    .into()
}

/// The shared store being unreachable is a server fault, which this operation models.
fn store_unavailable() -> PublishMetricsError {
    PublishMetricsErrorModeled::InternalServerException(InternalServerException {
        message: Some("the Amazon MWAA environment store is unavailable".to_string()),
    })
    .into()
}

/// The dotted path of one element of `MetricData`, spelled as the boundary validator spells
/// a member path: lowerCamel member names, and a list element naming its 1-based position
/// followed by the step into the collection. So a refusal this handler raises names the
/// offending datum the same way a refusal of a MODELLED constraint on it would.
fn datum_path(index: usize) -> String {
    format!("metricData.{}.member", index + 1)
}

/// The value of a required member of a statistic set, or the refusal naming it: a set
/// missing a member summarises no set of observations, so it cannot be published. (Every
/// member of the modelled `StatisticSet` is optional, so the boundary validator passes a
/// partial one.)
fn required<T>(path: &str, member: &str, value: Option<T>) -> Result<T, PublishMetricsError> {
    value.ok_or_else(|| validation(format!("{path}.{member} must be specified.")))
}

/// `value` when it is a number a published metric may carry, or the refusal naming the
/// member it arrived in. See [`is_publishable_metric_value`].
fn publishable(path: &str, value: f64) -> Result<f64, PublishMetricsError> {
    if is_publishable_metric_value(value) {
        return Ok(value);
    }
    Err(validation(format!(
        "{path} is not a metric value this service records: a value must be finite and no \
         greater in magnitude than 2^{}.",
        crate::state::MAX_METRIC_VALUE_EXPONENT
    )))
}

/// A metric name, dimension name or dimension value as it will be stored, or the refusal
/// naming the member it arrived in.
///
/// Blank is refused because a name made only of whitespace names nothing, and the length is
/// bounded because none of these members carries a modelled `@length` — see
/// [`crate::state::MAX_METRIC_LABEL_LENGTH`]. The text is stored exactly as sent: trimming it
/// would silently rename the caller's metric.
fn label(path: &str, value: &str) -> Result<String, PublishMetricsError> {
    if value.trim().is_empty() {
        return Err(validation(format!("{path} must not be blank.")));
    }
    if value.chars().count() > crate::state::MAX_METRIC_LABEL_LENGTH {
        return Err(validation(format!(
            "{path} must have length less than or equal to {}.",
            crate::state::MAX_METRIC_LABEL_LENGTH
        )));
    }
    Ok(value.to_string())
}

/// The observations a `StatisticValues` set states, or the refusal naming what is wrong with
/// it.
fn statistics_of_set(
    path: &str,
    set: &StatisticSet,
) -> Result<MetricStatistics, PublishMetricsError> {
    let path = format!("{path}.statisticValues");
    let sample_count = required(&path, "sampleCount", set.sample_count)?;
    let sum = publishable(
        &format!("{path}.sum"),
        required(&path, "sum", set.sum)?,
    )?;
    let minimum = publishable(
        &format!("{path}.minimum"),
        required(&path, "minimum", set.minimum)?,
    )?;
    let maximum = publishable(
        &format!("{path}.maximum"),
        required(&path, "maximum", set.maximum)?,
    )?;
    // A set of no observations summarises nothing, and a negative count counts nothing.
    if sample_count <= 0 {
        return Err(validation(format!(
            "{path}.sampleCount must be greater than 0."
        )));
    }
    // A summary whose smallest observation is larger than its largest describes no set of
    // observations.
    if minimum > maximum {
        return Err(validation(format!(
            "{path}.minimum must be less than or equal to {path}.maximum."
        )));
    }
    Ok(MetricStatistics {
        sample_count: i64::from(sample_count),
        sum,
        minimum,
        maximum,
    })
}

/// The dimension set one datum names, as the series key holds it, or the refusal naming what
/// is wrong with it.
fn dimensions_of(
    path: &str,
    dimensions: Option<&Vec<Dimension>>,
) -> Result<BTreeMap<String, String>, PublishMetricsError> {
    let dimensions = dimensions.map_or(&[][..], Vec::as_slice);
    if dimensions.len() > crate::state::MAX_METRIC_DIMENSIONS {
        return Err(validation(format!(
            "{path}.dimensions must have length less than or equal to {}.",
            crate::state::MAX_METRIC_DIMENSIONS
        )));
    }
    let mut resolved: BTreeMap<String, String> = BTreeMap::new();
    for (position, dimension) in dimensions.iter().enumerate() {
        let member = format!("{path}.dimensions.{}.member", position + 1);
        let name = label(&format!("{member}.name"), &dimension.name)?;
        let value = label(&format!("{member}.value"), &dimension.value)?;
        // A dimension set is a map from name to value, so a name stated twice would collapse
        // into one and silently discard a value the caller sent.
        if resolved.insert(name.clone(), value).is_some() {
            return Err(validation(format!(
                "{path}.dimensions names the dimension {name} more than once."
            )));
        }
    }
    Ok(resolved)
}

/// One datum of the request as it will be stored, or the refusal naming what is wrong with
/// it.
fn published(index: usize, datum: &MetricDatum) -> Result<PublishedMetric, PublishMetricsError> {
    let path = datum_path(index);
    let metric_name = label(&format!("{path}.metricName"), &datum.metric_name)?;
    // `DateTime` owns the conversion, including its overflow check, so an instant this store
    // cannot hold is refused rather than recorded at the wrong one.
    let timestamp_millis = datum.timestamp.to_millis().map_err(|_| {
        validation(format!(
            "{path}.timestamp is not an instant this service can record."
        ))
    })?;
    let statistics = match (datum.value, datum.statistic_values.as_ref()) {
        (Some(_), Some(_)) => {
            return Err(validation(format!(
                "{path}.value and {path}.statisticValues are mutually exclusive: a metric \
                 datum states its observations as one or the other."
            )))
        }
        (None, None) => {
            return Err(validation(format!(
                "either {path}.value or {path}.statisticValues must be specified."
            )))
        }
        (Some(value), None) => {
            MetricStatistics::of_value(publishable(&format!("{path}.value"), value)?)
        }
        (None, Some(set)) => statistics_of_set(&path, set)?,
    };
    Ok(PublishedMetric {
        metric_name,
        dimensions: dimensions_of(&path, datum.dimensions.as_ref())?,
        datapoint: MetricDatapoint {
            timestamp_millis,
            unit: datum.unit.clone(),
            statistics,
        },
    })
}

/// The `PublishMetrics` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct PublishMetrics<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> PublishMetrics<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for PublishMetrics<Eng>
{
    type Input = PublishMetricsInput;
    type Output = PublishMetricsOutput;
    type Error = PublishMetricsError;

    const NAME: &'static str = "PublishMetrics";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUBLISH_METRICS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::PUBLISH_METRICS_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        let now = ::std::time::SystemTime::now();
        // An environment lives in the Region it was created in, so a name is resolved in the
        // Region this request was signed for and nowhere else.
        let region = crate::state::request_region(ctx);

        // Judged on the request alone, so the whole batch is refused before the store is
        // touched: a request carrying one datum the service cannot publish publishes none of
        // them.
        let batch = input
            .metric_data
            .iter()
            .enumerate()
            .map(|(index, datum)| published(index, datum))
            .collect::<Result<Vec<PublishedMetric>, PublishMetricsError>>()?;

        let state = crate::state::service_state(&*self.engine).ok_or_else(store_unavailable)?;
        let mut store = state.lock();
        store.settle(now);

        // Nothing resolved this name before the request reached here (the operation is
        // hosted on the `ops.` endpoint, not behind the per-environment front door), so this
        // layer owns the refusal: the shared sentence for a named environment it holds no
        // record of, in the only client error this operation models.
        let stored = store
            .environment_mut(&region, &input.environment_name)
            .ok_or_else(|| {
                validation(crate::state::environment_not_found_message(
                    &input.environment_name,
                ))
            })?;

        // An environment on its way out takes no change: what was published would be applied
        // to a record the clock is about to remove, so it would vanish with it.
        if let Some(message) = stored.change_refusal() {
            return Err(validation(message));
        }

        // All or nothing: the store checks the series ceiling against the whole batch before
        // it writes any of it.
        stored
            .metrics
            .record_all(&batch)
            .map_err(|rejection| match rejection {
                MetricRejection::SeriesLimitExceeded => validation(format!(
                    "this emulator holds at most {} metric series for one environment",
                    crate::state::MAX_METRIC_SERIES
                )),
            })?;

        Ok(PublishMetricsOutput {})
    }
}
