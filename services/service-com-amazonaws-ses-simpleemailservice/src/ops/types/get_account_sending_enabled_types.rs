// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_account_sending_enabled` operation.
//!
//! HAND-AUTHORED, like `get_send_quota_types`: the scaffold op-stub generator
//! declined `GetAccountSendingEnabled` because the model binds it to the
//! `smithy.api#Unit` input shape, which the generator does not emit yet. The two
//! shapes below follow exactly the pattern a rendered sibling uses, so the
//! operation is typed, validated against the model and routed like every other.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ses-simpleemailservice.json");

/// `GetAccountSendingEnabled`'s input. The operation's modeled input is
/// `smithy.api#Unit`, so the request carries no members of its own — only the
/// query protocol's `Action` and `Version` fields, which the protocol reads.
///
/// Declared with the `input = ` key rather than `shape = `: `shape = ` resolves
/// its kind by looking the id up in the service model, and `smithy.api#Unit` is
/// a Smithy PRELUDE shape, so it is not in there to look up.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetAccountSendingEnabledInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.ses#GetAccountSendingEnabledResponse")]
#[derive(Debug, Clone, Default)]
pub struct GetAccountSendingEnabledOutput {
    pub enabled: bool,
}
