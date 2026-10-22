// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_compliance_summary_by_config_rule`
//! operation.
//!
//! HAND-AUTHORED, unlike its siblings: the scaffold op-stub generator declined
//! `GetComplianceSummaryByConfigRule` because the model binds it to the
//! `smithy.api#Unit` input shape, which the generator does not emit yet (see
//! `src/ops/SKIPPED_STUBS.md`). The two shapes below follow exactly the pattern a
//! rendered sibling uses, so the operation is typed, validated against the model and
//! routed like every other.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-configservice-starlingdoveservice.json");

/// `GetComplianceSummaryByConfigRule`'s input. The operation's modeled input is
/// `smithy.api#Unit`, so the request carries no members of its own.
///
/// Declared with the `input = ` key rather than `shape = `: `shape = ` resolves its
/// kind by looking the id up in the service model, and `smithy.api#Unit` is a Smithy
/// PRELUDE shape, so it is not in there to look up. An empty input needs no lookup —
/// there are no members to check against the model.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetComplianceSummaryByConfigRuleInput {}

#[smithy_macros::smithy(
    shape = "com.amazonaws.configservice#GetComplianceSummaryByConfigRuleResponse"
)]
#[derive(Debug, Clone, Default)]
pub struct GetComplianceSummaryByConfigRuleOutput {
    pub compliance_summary: Option<ComplianceSummary>,
}
