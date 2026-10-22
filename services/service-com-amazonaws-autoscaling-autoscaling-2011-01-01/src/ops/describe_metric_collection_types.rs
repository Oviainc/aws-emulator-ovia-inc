// SPDX-License-Identifier: CC0-1.0

//! @op DescribeMetricCollectionTypes
//!
//! Reports the catalog of group metrics and granularities metrics collection can be
//! enabled for: one `MetricCollectionType` per metric and one
//! `MetricGranularityType` per granularity, which is what a client validates an
//! `EnableMetricsCollection`/`DisableMetricsCollection` argument against.
//!
//! The catalog is SERVICE data rather than per-account state — the same answer for
//! every caller, in every region, whatever the store holds — so this handler takes
//! no lock and writes nothing. Its membership and ordering live in
//! [`crate::state::METRIC_COLLECTION_TYPES`] and
//! [`crate::state::METRIC_GRANULARITIES`], which name the model members that
//! publish them.
//!
//! HAND-AUTHORED. The op-stub generator declined to emit a stub for this operation
//! because the model binds it to Smithy's `Unit` input, which it cannot type yet
//! (see `src/ops/SKIPPED_STUBS.md`): it had no handler file, so it was absent from
//! the generated `operations(...)` allow-list and every well-formed call fell
//! through to the framework's unroutable-operation path — an HTTP 400 that is not
//! one of this operation's modelled outcomes. `build.rs` derives that allow-list
//! from the `//! @op <Name>` header of every `src/ops/*.rs`, so this file is what
//! wires it. Its input and output shapes live in
//! `types/describe_metric_collection_types_types.rs`.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request.
//! Reach the store through [`crate::state::service_state`]; never construct an
//! engine in a handler. This operation needs no store, so it touches neither.
//!
//! ---
//!
//! # Modeled contract for `DescribeMetricCollectionTypes`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation",operation="DescribeMetricCollectionTypes",model_path="services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01/smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json")`):
//!
//! Describes the available CloudWatch metrics for Amazon EC2 Auto Scaling.
//!
//! ## Input
//!
//! (no members — `smithy.api#Unit`)
//!
//! ## Output
//!
//! * `Metrics` (MetricCollectionTypes) — The metrics.
//! * `Granularities` (MetricGranularityTypes) — The granularities for the metrics.
//!
//! ## Errors
//!
//! * `ResourceContentionFault` (HTTP 500) — You already have a pending update to an
//!   Amazon EC2 Auto Scaling resource (for example, an Auto Scaling group, instance,
//!   or load balancer).

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json");

/// The operation's MODELED errors. `#[operation_error]` accepts only variants the
/// model declares for this operation.
#[smithy_macros::operation_error("com.amazonaws.autoscaling#DescribeMetricCollectionTypes")]
#[derive(Debug)]
pub enum DescribeMetricCollectionTypesErrorModeled {
    ResourceContentionFault(ResourceContentionFault),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::describe_metric_collection_types::DescribeMetricCollectionTypesError`).
pub type DescribeMetricCollectionTypesError =
    crate::scaffolded::Scaffolded<DescribeMetricCollectionTypesErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<DescribeMetricCollectionTypesErrorModeled> for DescribeMetricCollectionTypesError {
    fn from(modeled: DescribeMetricCollectionTypesErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `DescribeMetricCollectionTypes` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so
/// this crate names no concrete engine type.
pub struct DescribeMetricCollectionTypes<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> DescribeMetricCollectionTypes<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency. Called by the generated `EmulatedService::new`.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for DescribeMetricCollectionTypes<Eng>
{
    type Input = DescribeMetricCollectionTypesInput;
    type Output = DescribeMetricCollectionTypesOutput;
    type Error = DescribeMetricCollectionTypesError;

    const NAME: &'static str = "DescribeMetricCollectionTypes";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_METRIC_COLLECTION_TYPES_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema =
        super::types::DESCRIBE_METRIC_COLLECTION_TYPES_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The input is `smithy.api#Unit`, so there is nothing to read from it, and
        // the catalog is the same for every caller: no request metadata and no store
        // entry takes part in the answer.
        let _ = (&input, ctx, &self.engine);

        Ok(DescribeMetricCollectionTypesOutput {
            metrics: Some(
                crate::state::METRIC_COLLECTION_TYPES
                    .iter()
                    .map(|metric| MetricCollectionType {
                        metric: Some((*metric).to_string()),
                    })
                    .collect(),
            ),
            granularities: Some(
                crate::state::METRIC_GRANULARITIES
                    .iter()
                    .map(|granularity| MetricGranularityType {
                        granularity: Some((*granularity).to_string()),
                    })
                    .collect(),
            ),
        })
    }
}
