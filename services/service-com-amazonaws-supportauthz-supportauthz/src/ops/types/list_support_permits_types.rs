// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_support_permits` operation.
//!
//! Hand-authored: the scaffold declined `ListSupportPermits` (see
//! `src/ops/SKIPPED_STUBS.md`), so these follow the shape the scaffold gives the
//! rendered siblings in this directory. Each structure/union is declared exactly
//! once across this service's `types/` modules, and `types/mod.rs` re-exports
//! them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-supportauthz-supportauthz.json");

#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#ListSupportPermitsInput")]
#[derive(Debug, Clone, Default)]
pub struct ListSupportPermitsInput {
    pub next_token: Option<String>,
    pub max_results: Option<i32>,
    pub support_permit_statuses: Option<Vec<String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.supportauthz#ListSupportPermitsOutput")]
#[derive(Debug, Clone, Default)]
pub struct ListSupportPermitsOutput {
    pub support_permits: Vec<SupportPermitSummary>,
    pub next_token: Option<String>,
}
