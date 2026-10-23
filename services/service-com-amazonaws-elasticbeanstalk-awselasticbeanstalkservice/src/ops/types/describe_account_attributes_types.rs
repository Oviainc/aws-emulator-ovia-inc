// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_account_attributes` operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice.json");

/// `DescribeAccountAttributes` takes no input: the operation's modelled input is
/// `smithy.api#Unit`. The empty shape still carries a schema, a serializer and a
/// deserializer, because the framework's centralized request walk deserializes and
/// validates every input against `INPUT_SCHEMA` before `handle` runs.
#[smithy_macros::smithy(input = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DescribeAccountAttributesInput;

#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#DescribeAccountAttributesResult")]
#[derive(Debug, Clone, Default)]
pub struct DescribeAccountAttributesOutput {
    pub resource_quotas: Option<ResourceQuotas>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#ResourceQuotas")]
#[derive(Debug, Clone, Default)]
pub struct ResourceQuotas {
    pub application_quota: Option<ResourceQuota>,
    pub application_version_quota: Option<ResourceQuota>,
    pub environment_quota: Option<ResourceQuota>,
    pub configuration_template_quota: Option<ResourceQuota>,
    pub custom_platform_quota: Option<ResourceQuota>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#ResourceQuota")]
#[derive(Debug, Clone, Default)]
pub struct ResourceQuota {
    pub maximum: Option<i32>,
}
