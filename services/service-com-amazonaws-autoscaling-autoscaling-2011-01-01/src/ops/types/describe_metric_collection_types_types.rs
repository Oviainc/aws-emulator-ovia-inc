// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_metric_collection_types` operation.
//!
//! HAND-AUTHORED, not scaffolded: `DescribeMetricCollectionTypes` binds Smithy's
//! `Unit` input, which the op-stub generator declines to type (see
//! `src/ops/SKIPPED_STUBS.md`), so its shapes are declared here beside the
//! generated ones. Each structure/union is declared exactly once across this
//! service's `types/` modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json");

/// `DescribeMetricCollectionTypes` has no input members — see
/// [`super::describe_account_limits_types::DescribeAccountLimitsInput`] for why a
/// Unit input is declared with `structure = "smithy.api#Unit"`.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DescribeMetricCollectionTypesInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.autoscaling#DescribeMetricCollectionTypesAnswer")]
#[derive(Debug, Clone, Default)]
pub struct DescribeMetricCollectionTypesOutput {
    pub metrics: Option<Vec<MetricCollectionType>>,
    pub granularities: Option<Vec<MetricGranularityType>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.autoscaling#MetricCollectionType")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetricCollectionType {
    pub metric: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.autoscaling#MetricGranularityType")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MetricGranularityType {
    pub granularity: Option<String>,
}
