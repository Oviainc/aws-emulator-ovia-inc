// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `create_storage_location` operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json");

/// `CreateStorageLocation` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct CreateStorageLocationInput;

#[smithy_macros::smithy(
    shape = "com.amazonaws.elasticbeanstalk#CreateStorageLocationResultMessage"
)]
#[derive(Debug, Clone, Default)]
pub struct CreateStorageLocationOutput {
    pub s3_bucket: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#S3SubscriptionRequiredException")]
#[derive(Debug, Clone, Default)]
pub struct S3SubscriptionRequiredException {
    pub message: Option<String>,
}
