// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_service_settings` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `GetServiceSettings`
//! because its input is `smithy.api#Unit`, a construct it does not emit yet (it was
//! recorded in `src/ops/SKIPPED_STUBS.md`). Everything here follows the pattern of a
//! rendered sibling `_types` module.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json");

/// The operation's input: `smithy.api#Unit`, i.e. no members at all.
///
/// A member-less struct is what the framework expects for a Unit input, and
/// `structure = "…"` (not `shape = "…"`) is how it is declared: the empty-struct path
/// emits the `SCHEMA`, the `GET_SERVICE_SETTINGS_INPUT_SCHEMA` static and both the
/// serialize and deserialize impls `Operation::Input` requires, without looking the
/// shape up — `smithy.api#Unit` is a prelude shape that no service model declares.
/// Keeping the prelude shape id is what makes the restJson1 codec treat the input as
/// having no body.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetServiceSettingsInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.ssmquicksetup#GetServiceSettingsOutput")]
#[derive(Debug, Clone, Default)]
pub struct GetServiceSettingsOutput {
    pub service_settings: Option<ServiceSettings>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.ssmquicksetup#ServiceSettings")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServiceSettings {
    pub explorer_enabling_role_arn: Option<String>,
}
