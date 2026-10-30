// SPDX-License-Identifier: CC0-1.0

//! The `ResourceNotFoundException` the dynamic-instrumentation operations serialize.
//!
//! One shape, two wire bodies. A refusal about a SERVICE LEVEL OBJECTIVE carries the
//! resource's identity — `ResourceId` and `ResourceType` beside `Message` — and is built
//! from the shape declared in `shared_types`. A refusal about an INSTRUMENTATION
//! CONFIGURATION carries `Message` alone: the request already names the identity it asked
//! about (service, environment, signal type and location), so the body states the fact and
//! nothing else, and it names no ARN. The model marks all three members `@required`, which
//! leaves a `@required` member non-`Option` and therefore always written, so the only way to
//! keep a member OFF the wire is to declare it unsupported — which is what this module's
//! copy of the shape does.
//!
//! This module is deliberately NOT glob-re-exported by `types/mod.rs`: both copies carry the
//! Rust name the operation error enums must use (`ResourceNotFoundException`, the name of
//! the shape), so two globs exporting it would make every use of the name ambiguous. An
//! operation that serializes this copy imports it by path, which shadows the glob.

#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-applicationsignals-applicationsignals.json");

/// `ResourceNotFoundException` as the instrumentation-configuration operations put it on the
/// wire: `Message` only.
///
/// `ResourceType` and `ResourceId` are declared unsupported, so they are absent from this
/// shape's schema and from the body the framework writes, rather than being filled with an
/// ARN and a type name the recorded refusals of these operations do not carry.
#[smithy_macros::smithy(
    shape = "com.amazonaws.applicationsignals#ResourceNotFoundException",
    unsupported = ["ResourceType", "ResourceId"]
)]
#[derive(Debug, Clone, Default)]
pub struct ResourceNotFoundException {
    pub message: String,
}
