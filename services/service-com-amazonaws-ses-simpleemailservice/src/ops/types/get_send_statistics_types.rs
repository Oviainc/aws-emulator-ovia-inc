// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_send_statistics` operation.
//!
//! HAND-AUTHORED, like `get_send_quota_types`: the scaffold op-stub generator
//! declined `GetSendStatistics` because the model binds it to the
//! `smithy.api#Unit` input shape, which the generator does not emit yet. The
//! shapes below follow exactly the pattern a rendered sibling uses, so the
//! operation is typed, validated against the model and routed like every other.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ses-simpleemailservice.json");

/// `GetSendStatistics`' input. The operation's modeled input is
/// `smithy.api#Unit`, so the request carries no members of its own — only the
/// query protocol's `Action` and `Version` fields, which the protocol reads.
///
/// Declared with the `input = ` key rather than `shape = `: `shape = ` resolves
/// its kind by looking the id up in the service model, and `smithy.api#Unit` is
/// a Smithy PRELUDE shape, so it is not in there to look up.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct GetSendStatisticsInput {}

#[smithy_macros::smithy(shape = "com.amazonaws.ses#GetSendStatisticsResponse")]
#[derive(Debug, Clone, Default)]
pub struct GetSendStatisticsOutput {
    pub send_data_points: Option<Vec<SendDataPoint>>,
}

/// `SendDataPoint` — "Represents sending statistics data. Each `SendDataPoint`
/// contains statistics for a 15-minute period of sending activity."
///
/// The four counters target `Counter`, a `long` whose model default is `0`, so
/// each is a plain `i64`: a period with none of that event reports zero rather
/// than omitting the member.
#[smithy_macros::smithy(shape = "com.amazonaws.ses#SendDataPoint")]
#[derive(Debug, Clone, Default)]
pub struct SendDataPoint {
    pub timestamp: Option<::aws_smithy_types::DateTime>,
    pub delivery_attempts: i64,
    pub bounces: i64,
    pub complaints: i64,
    pub rejects: i64,
}
