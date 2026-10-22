// SPDX-License-Identifier: CC0-1.0

//! Type definitions for this service's operation.
//!
//! The scaffold generator declined this service (its single operation has a
//! `@streaming` event-stream input member), so these shapes are hand-authored
//! rather than generated — but they are declared exactly the way a generated
//! shape is, with `#[smithy_macros::smithy(shape = ...)]` reading the Smithy
//! model, so each one's wire names, HTTP bindings and constraint traits come
//! from the model and not from this file.
//!
//! Every submodule is re-exported here, so `use super::types::*` resolves a type
//! wherever it lives.

pub mod invoke_endpoint_with_bidirectional_stream_types;

#[allow(ambiguous_glob_reexports, unused_imports)]
pub use invoke_endpoint_with_bidirectional_stream_types::*;
