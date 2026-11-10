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

/// The account's Elastic Beanstalk quotas: the modelled `ResourceQuotas`, plus
/// `HealthGroupQuota`.
///
/// Member set and order from
/// `smithy_query(query="shape", shape="ResourceQuotas")`, with
/// `HealthGroupQuota` — RECORDED BEHAVIOUR rather than modelled — appended last.
/// See `types::unmodelled_member_wire` for why the shape is hand-written.
#[derive(Debug, Clone, Default)]
pub struct ResourceQuotas {
    pub application_quota: Option<ResourceQuota>,
    pub application_version_quota: Option<ResourceQuota>,
    pub environment_quota: Option<ResourceQuota>,
    pub configuration_template_quota: Option<ResourceQuota>,
    pub custom_platform_quota: Option<ResourceQuota>,
    pub health_group_quota: Option<ResourceQuota>,
}

eb_schema_and_codec!(
    ResourceQuotas,
    RESOURCE_QUOTAS_SCHEMA,
    "com.amazonaws.elasticbeanstalk#ResourceQuotas",
    "com.amazonaws.elasticbeanstalk",
    "ResourceQuotas",
    (
        M_APPLICATION_QUOTA,
        application_quota,
        "ApplicationQuota",
        0,
        opt,
        struct_of(ResourceQuota)
    ),
    (
        M_APPLICATION_VERSION_QUOTA,
        application_version_quota,
        "ApplicationVersionQuota",
        1,
        opt,
        struct_of(ResourceQuota)
    ),
    (
        M_ENVIRONMENT_QUOTA,
        environment_quota,
        "EnvironmentQuota",
        2,
        opt,
        struct_of(ResourceQuota)
    ),
    (
        M_CONFIGURATION_TEMPLATE_QUOTA,
        configuration_template_quota,
        "ConfigurationTemplateQuota",
        3,
        opt,
        struct_of(ResourceQuota)
    ),
    (
        M_CUSTOM_PLATFORM_QUOTA,
        custom_platform_quota,
        "CustomPlatformQuota",
        4,
        opt,
        struct_of(ResourceQuota)
    ),
    (
        M_HEALTH_GROUP_QUOTA,
        health_group_quota,
        "HealthGroupQuota",
        5,
        opt,
        struct_of(ResourceQuota)
    ),
);

#[smithy_macros::smithy(shape = "com.amazonaws.elasticbeanstalk#ResourceQuota")]
#[derive(Debug, Clone, Default)]
pub struct ResourceQuota {
    pub maximum: Option<i32>,
}
