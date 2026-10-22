// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_account_limits` operation.
//!
//! HAND-AUTHORED, not scaffolded: `DescribeAccountLimits` binds Smithy's `Unit`
//! input, which the op-stub generator declines to type (see
//! `src/ops/SKIPPED_STUBS.md`), so its shapes are declared here beside the
//! generated ones. Each structure/union is declared exactly once across this
//! service's `types/` modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json");

/// `DescribeAccountLimits` has no input members: the model binds it to
/// `smithy.api#Unit`, a PRELUDE shape that is not in this service's model file.
/// The legacy `structure = "..."` key declares the shape id without resolving it
/// against the model, and the macro's empty-struct path emits the schema, the
/// `DESCRIBE_ACCOUNT_LIMITS_INPUT_SCHEMA` static and the serialize/deserialize
/// impls `Operation::Input` requires from the id alone.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DescribeAccountLimitsInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.autoscaling#DescribeAccountLimitsAnswer")]
#[derive(Debug, Clone, Default)]
pub struct DescribeAccountLimitsOutput {
    pub max_number_of_auto_scaling_groups: Option<i32>,
    pub max_number_of_launch_configurations: Option<i32>,
    pub number_of_auto_scaling_groups: Option<i32>,
    pub number_of_launch_configurations: Option<i32>,
}
