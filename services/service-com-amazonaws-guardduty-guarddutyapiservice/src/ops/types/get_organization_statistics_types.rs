// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_organization_statistics` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `GetOrganizationStatistics`
//! because its input is `smithy.api#Unit`, a construct it does not emit yet (it was
//! recorded in `src/ops/SKIPPED_STUBS.md`). Everything here follows the pattern of a
//! rendered sibling `_types` module.
//!
//! Each structure is declared exactly once across this service's `types/` modules, and
//! `types/mod.rs` re-exports them all: the four output shapes below
//! (`OrganizationDetails` and the statistics tree under it) are reached by no other
//! operation, so this is where they live.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-guardduty-guarddutyapiservice.json");

/// The operation's input: `smithy.api#Unit`, i.e. no members at all.
///
/// A member-less struct is what the framework expects for a Unit input, and
/// `structure = "…"` (not `shape = "…"`) is how it is declared: the empty-struct path
/// emits the `SCHEMA`, the `GET_ORGANIZATION_STATISTICS_INPUT_SCHEMA` static and both
/// the serialize and deserialize impls `Operation::Input` requires, without looking the
/// shape up — `smithy.api#Unit` is a prelude shape that no service model declares.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetOrganizationStatisticsInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#GetOrganizationStatisticsResponse")]
#[derive(Debug, Clone, Default)]
pub struct GetOrganizationStatisticsOutput {
    pub organization_details: Option<OrganizationDetails>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#OrganizationDetails")]
#[derive(Debug, Clone, Default)]
pub struct OrganizationDetails {
    pub updated_at: Option<::aws_smithy_types::DateTime>,
    pub organization_statistics: Option<OrganizationStatistics>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#OrganizationStatistics")]
#[derive(Debug, Clone, Default)]
pub struct OrganizationStatistics {
    pub total_accounts_count: Option<i32>,
    pub member_accounts_count: Option<i32>,
    pub active_accounts_count: Option<i32>,
    pub enabled_accounts_count: Option<i32>,
    pub count_by_feature: Option<Vec<OrganizationFeatureStatistics>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.guardduty#OrganizationFeatureStatistics")]
#[derive(Debug, Clone, Default)]
pub struct OrganizationFeatureStatistics {
    pub name: Option<String>,
    pub enabled_accounts_count: Option<i32>,
    pub additional_configuration: Option<Vec<OrganizationFeatureStatisticsAdditionalConfiguration>>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.guardduty#OrganizationFeatureStatisticsAdditionalConfiguration"
)]
#[derive(Debug, Clone, Default)]
pub struct OrganizationFeatureStatisticsAdditionalConfiguration {
    pub name: Option<String>,
    pub enabled_accounts_count: Option<i32>,
}
