// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `disassociate_default_view` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `DisassociateDefaultView`
//! because its input is `smithy.api#Unit`, a construct it does not emit yet (it was
//! recorded in `src/ops/SKIPPED_STUBS.md`). Everything here follows the pattern of a
//! rendered sibling `_types` module.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-resourceexplorer2-resourceexplorer.json");

/// The operation's input: `smithy.api#Unit`, i.e. no members at all. Which Region's
/// default view is cleared comes from the request's SigV4 credential scope, not from a
/// member.
///
/// Declared with `structure = "…"` rather than `shape = "…"` for the reason given on
/// `GetIndexInput`: `smithy.api#Unit` is a Smithy prelude shape that no service model
/// carries, and an empty structure has no members to validate against one.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DisassociateDefaultViewInput {}

/// The operation's output: `smithy.api#Unit` as well — the call reports nothing beyond
/// its success, so a success renders as an empty body with the modelled 200.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DisassociateDefaultViewOutput {}
