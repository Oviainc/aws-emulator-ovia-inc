// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_quick_setup_types` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `ListQuickSetupTypes`
//! because its input is `smithy.api#Unit`, a construct it does not emit yet (it was
//! recorded in `src/ops/SKIPPED_STUBS.md`). Everything here follows the pattern of a
//! rendered sibling `_types` module.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssmquicksetup-quicksetup.json");

/// The operation's input: `smithy.api#Unit`, i.e. no members at all. Declared with
/// `structure = "smithy.api#Unit"` for the reason
/// `get_service_settings_types::GetServiceSettingsInput` documents.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct ListQuickSetupTypesInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.ssmquicksetup#ListQuickSetupTypesOutput")]
#[derive(Debug, Clone, Default)]
pub struct ListQuickSetupTypesOutput {
    pub quick_setup_type_list: Option<Vec<QuickSetupTypeOutput>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.ssmquicksetup#QuickSetupTypeOutput")]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QuickSetupTypeOutput {
    pub r#type: Option<String>,
    pub latest_version: Option<String>,
}
