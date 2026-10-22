// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `delete_grouping_configuration` operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-applicationsignals-applicationsignals.json");

/// `DeleteGroupingConfiguration`'s input.
///
/// The operation's modelled input is `smithy.api#Unit`
/// (`smithy_query(query="operation",operation="DeleteGroupingConfiguration")` ->
/// `input_shape: "Unit"`): it takes NOTHING, neither a body member nor an HTTP binding, so
/// the Rust input is an empty structure carrying the `smithy.api#Unit` shape id. The
/// framework needs a named input type to deserialize into even when the wire carries no
/// members.
///
/// Declared with the `input = ...` key rather than `shape = ...`: `shape` resolves the
/// Rust item kind by looking the shape up in THIS service's model, and `smithy.api#Unit` is
/// a Smithy prelude shape that no service model carries. An empty structure needs no shape
/// definition anyway — it has no members to validate against one.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DeleteGroupingConfigurationInput {}

/// `DeleteGroupingConfiguration`'s output: the modelled `DeleteGroupingConfigurationOutput`
/// structure, which declares no members, so a success renders as the empty JSON body `{}`
/// with the operation's modelled success code of 200.
#[smithy_macros::smithy(shape = "com.amazonaws.applicationsignals#DeleteGroupingConfigurationOutput")]
#[derive(Debug, Clone, Default)]
pub struct DeleteGroupingConfigurationOutput {}
