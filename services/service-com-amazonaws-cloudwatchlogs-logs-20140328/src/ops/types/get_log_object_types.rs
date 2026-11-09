// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_log_object` operation.
//!
//! Hand-authored rather than generated: `GetLogObject`'s output member `fieldStream`
//! targets a `@streaming` union, so the op-stub generator emitted neither the operation
//! nor its shapes (`src/ops/SKIPPED_STUBS.md`).
//!
//! Only the REQUEST shape is declared, in full, so the framework's own deserializer fills
//! it from the awsJson body and `constraints::validate` checks it against the model's
//! `@length` traits — the request half is judged by the model, not by hand.
//! `GetLogObjectResponse`'s only member is the `@streaming` payload, so there are no
//! modelled response headers to serialize, and the union's `fields` member is constructed
//! by nothing while this store holds no large log object
//! ([`crate::ops::get_log_object`]).
//!
//! Each structure is declared exactly once across this service's `types/` modules, and
//! `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudwatchlogs-logs-20140328.json");

/// `GetLogObjectRequest`: which large log object to stream, and whether its data
/// protection masking is lifted.
#[smithy_macros::smithy(shape = "com.amazonaws.cloudwatchlogs#GetLogObjectRequest")]
#[derive(Debug, Clone, Default)]
pub struct GetLogObjectInput {
    pub log_object_pointer: String,
    pub unmask: bool,
}
