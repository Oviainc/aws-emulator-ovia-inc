// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_global_settings` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `GetGlobalSettings` because
//! its input is `smithy.api#Unit`, a construct it does not emit yet (it was recorded in
//! `src/ops/SKIPPED_STUBS.md`). Everything here follows the pattern of a rendered
//! sibling `_types` module.
//!
//! `AccountJiraConfigurationInput` — the shape `UpdateGlobalSettings` writes the account's
//! Jira settings with — is declared by `update_global_settings_types` and re-exported by
//! `types/mod.rs`, so it is NOT redeclared here: each structure is declared exactly once
//! across this service's `types/` modules. The OUTPUT counterpart below is reached only
//! by this operation, so it lives here.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-wellarchitected-wellarchitectedapiservicelambda.json");

/// The operation's input: `smithy.api#Unit`, i.e. no members at all.
///
/// A member-less struct is what the framework expects for a Unit input, and
/// `structure = "…"` (not `shape = "…"`) is how it is declared: the empty-struct path
/// emits the `SCHEMA`, the `GET_GLOBAL_SETTINGS_INPUT_SCHEMA` static and both the
/// serialize and deserialize impls `Operation::Input` requires, without looking the
/// shape up — `smithy.api#Unit` is a prelude shape that no service model declares.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetGlobalSettingsInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.wellarchitected#GetGlobalSettingsOutput")]
#[derive(Debug, Clone, Default)]
pub struct GetGlobalSettingsOutput {
    pub organization_sharing_status: Option<String>,
    pub discovery_integration_status: Option<String>,
    pub jira_configuration: Option<AccountJiraConfigurationOutput>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.wellarchitected#AccountJiraConfigurationOutput")]
#[derive(Debug, Clone, Default)]
pub struct AccountJiraConfigurationOutput {
    pub integration_status: Option<String>,
    pub issue_management_status: Option<String>,
    pub issue_management_type: Option<String>,
    pub subdomain: Option<String>,
    pub jira_project_key: Option<String>,
    pub status_message: Option<String>,
}
