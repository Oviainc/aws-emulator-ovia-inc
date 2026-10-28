// SPDX-License-Identifier: CC0-1.0

//! EventBridge event patterns, as this service reads and matches them: the
//! shared reading of the pattern language ([`::service_common::event_pattern`]).
//!
//! The language is not this service's own — an EventBridge rule's `EventPattern`
//! and a Pipes `FilterCriteria` filter are patterns of the SAME language — so
//! parsing, structural validation and matching live once in the shared crate and
//! this module is the name this service reaches them by. `PutRule` and
//! `TestEventPattern` run [`parse`], so anything the store holds is a pattern
//! [`matches_within`] understands, and `PutEvents` spends one [`MatchBudget`]
//! across the rules of a request.

pub use ::service_common::event_pattern::{
    matches_within, parse, MatchBudget, MAX_MATCH_STEPS_PER_REQUEST, MAX_PATTERN_BYTES,
    MAX_PATTERN_DEPTH,
};
