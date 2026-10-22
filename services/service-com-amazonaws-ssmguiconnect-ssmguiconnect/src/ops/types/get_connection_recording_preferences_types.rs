// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_connection_recording_preferences` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined
//! `GetConnectionRecordingPreferences` because its input is `smithy.api#Unit`, a
//! construct it does not emit yet (it was recorded in `src/ops/SKIPPED_STUBS.md`).
//! Everything here follows the pattern of a rendered sibling `_types` module.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssmguiconnect-ssmguiconnect.json");

/// The operation's input: `smithy.api#Unit`, i.e. no members at all.
///
/// A member-less struct is what the framework expects for a Unit input, and
/// `structure = "…"` (not `shape = "…"`) is how it is declared: the empty-struct
/// path emits the `SCHEMA`, the
/// `GET_CONNECTION_RECORDING_PREFERENCES_INPUT_SCHEMA` static and both the
/// serialize and deserialize impls `Operation::Input` requires, without looking the
/// shape up — `smithy.api#Unit` is a prelude shape that no service model declares.
/// Keeping the prelude shape id is what makes the restJson1 codec treat the input
/// as having no body.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetConnectionRecordingPreferencesInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.ssmguiconnect#GetConnectionRecordingPreferencesResponse")]
#[derive(Debug, Clone, Default)]
pub struct GetConnectionRecordingPreferencesOutput {
    pub client_token: Option<String>,
    pub connection_recording_preferences: Option<ConnectionRecordingPreferences>,
}
