// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_account_settings` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `GetAccountSettings` because
//! its input is `smithy.api#Unit`, a construct it does not emit yet (it was recorded in
//! `src/ops/SKIPPED_STUBS.md`). Everything here follows the pattern of a rendered
//! sibling `_types` module.
//!
//! `DeletionProtectionSettings` and `VendedMetricsSettings` — the two shapes this
//! operation's output carries — are already declared by `update_account_settings_types`,
//! which writes them, and `types/mod.rs` re-exports them, so they are NOT redeclared
//! here: each structure is declared exactly once across this service's `types/` modules.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-appconfig-amazonappconfig.json");

/// The operation's input: `smithy.api#Unit`, i.e. no members at all.
///
/// A member-less struct is what the framework expects for a Unit input, and
/// `structure = "…"` (not `shape = "…"`) is how it is declared: the empty-struct path
/// emits the `SCHEMA`, the `GET_ACCOUNT_SETTINGS_INPUT_SCHEMA` static and both the
/// serialize and deserialize impls `Operation::Input` requires, without looking the
/// shape up — `smithy.api#Unit` is a prelude shape that no service model declares.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetAccountSettingsInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.appconfig#AccountSettings")]
#[derive(Debug, Clone, Default)]
pub struct GetAccountSettingsOutput {
    pub deletion_protection: Option<DeletionProtectionSettings>,
    pub vended_metrics: Option<VendedMetricsSettings>,
}
