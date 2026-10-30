// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_account_limits` operation.
//!
//! HAND-AUTHORED, not scaffolded: `DescribeAccountLimits` binds Smithy's `Unit`
//! input, which the op-stub generator declines to type (see
//! `src/ops/SKIPPED_STUBS.md`), so its shapes are declared here beside the
//! generated ones. Each structure/union is declared exactly once across this
//! service's `types/` modules, and `types/mod.rs` re-exports them all.
//!
//! # The answer carries a member the on-disk model does not declare
//!
//! `DescribeAccountLimitsAnswer` declares the four counts and nothing else
//! (`smithy_query(query="shape", shape="DescribeAccountLimitsAnswer")`), while the
//! service's own awsQuery response also carries the account's API call-rate bucket
//! as `ThrottlingLimits/AccountMaxCallRate`. `#[smithy_macros::smithy(shape = …)]`
//! derives a structure's schema, serializer and deserializer from the model's
//! member list and admits exactly that list, so a Rust field with no member behind
//! it is a compile error: the three structures below carry their schema and
//! serializer by hand instead, emitting the same items the derive emits — one
//! member schema per member (index = its position in the member list), the `SCHEMA`
//! naming them in that order, the `…_SCHEMA` static and the `SerializableStruct`
//! impl.
//!
//! The model's own members keep the indices and the order the model gives them, and
//! the unmodelled member is appended LAST, so a later model refresh that declares it
//! only has to move it into place and put the derive back. Deliberately absent, for
//! the same reason the derive's own output would be inert here: the constraint
//! traits (the validating walk runs on operation INPUTS only, and these shapes are
//! output-only), the `@http` chain (baked on only for an operation's input),
//! `@xmlName` (each member's wire element IS its Smithy member name) and
//! `DeserializableStruct` (nothing reads this answer back off the wire).
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-autoscaling-autoscaling-2011-01-01.json");

/// `DescribeAccountLimits` has no input members: the model binds it to
/// `smithy.api#Unit`, a PRELUDE shape that is not in this service's model file.
/// The legacy `structure = "..."` key declares the shape id without resolving it
/// against the model, and the macro's empty-struct path emits the schema, the
/// `DESCRIBE_ACCOUNT_LIMITS_INPUT_SCHEMA` static and the serialize/deserialize
/// impls `Operation::Input` requires from the id alone.
#[smithy_macros::smithy(structure = "smithy.api#Unit")]
#[derive(Debug, Clone, Default)]
pub struct DescribeAccountLimitsInput {}

/// The account's API call-rate bucket: how fast it refills, in calls per second,
/// and how many calls it holds.
///
/// RECORDED behaviour, not modelled — see this module's own documentation. Both
/// sub-members are reported from `crate::state`'s published figures.
#[derive(Debug, Clone, Default)]
pub struct AccountMaxCallRate {
    pub refill_rate: f64,
    pub bucket_size: i32,
}

#[allow(non_upper_case_globals)]
impl AccountMaxCallRate {
    pub const M_REFILL_RATE: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.autoscaling#AccountMaxCallRate$RefillRate",
                "com.amazonaws.autoscaling",
                "RefillRate",
            ),
            ::aws_smithy_schema::ShapeType::Double,
            "RefillRate",
            0,
        );

    pub const M_BUCKET_SIZE: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.autoscaling#AccountMaxCallRate$BucketSize",
                "com.amazonaws.autoscaling",
                "BucketSize",
            ),
            ::aws_smithy_schema::ShapeType::Integer,
            "BucketSize",
            1,
        );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.autoscaling#AccountMaxCallRate",
            "com.amazonaws.autoscaling",
            "AccountMaxCallRate",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[&Self::M_REFILL_RATE, &Self::M_BUCKET_SIZE],
    );
}

pub static ACCOUNT_MAX_CALL_RATE_SCHEMA: &::aws_smithy_schema::Schema = &AccountMaxCallRate::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for AccountMaxCallRate {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        // A rate is a DECIMAL on the wire, and the framework's XML writer renders a
        // floating-point value with Rust's own `to_string`, which drops the
        // fractional part of a whole one (`5` for `5.0`). The shared rendering of a
        // floating-point member for the query protocols is what writes the text, so
        // this member carries the decimal AWS writes without a second copy of that
        // rule living here.
        s.write_string(
            &Self::M_REFILL_RATE,
            &::service_common::query_decimal_floats::decimal_double_text(self.refill_rate),
        )?;
        s.write_integer(&Self::M_BUCKET_SIZE, self.bucket_size)?;
        Ok(())
    }
}

/// The throttling limits of the account, as `DescribeAccountLimits` reports them.
///
/// RECORDED behaviour, not modelled — see this module's own documentation.
#[derive(Debug, Clone, Default)]
pub struct ThrottlingLimits {
    pub account_max_call_rate: Option<AccountMaxCallRate>,
}

#[allow(non_upper_case_globals)]
impl ThrottlingLimits {
    pub const M_ACCOUNT_MAX_CALL_RATE: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.autoscaling#ThrottlingLimits$AccountMaxCallRate",
                "com.amazonaws.autoscaling",
                "AccountMaxCallRate",
            ),
            ::aws_smithy_schema::ShapeType::Structure,
            "AccountMaxCallRate",
            0,
        );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.autoscaling#ThrottlingLimits",
            "com.amazonaws.autoscaling",
            "ThrottlingLimits",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[&Self::M_ACCOUNT_MAX_CALL_RATE],
    );
}

pub static THROTTLING_LIMITS_SCHEMA: &::aws_smithy_schema::Schema = &ThrottlingLimits::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for ThrottlingLimits {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        if let ::std::option::Option::Some(rate) = self.account_max_call_rate.as_ref() {
            s.write_struct(&Self::M_ACCOUNT_MAX_CALL_RATE, rate)?;
        }
        Ok(())
    }
}

/// The answer: the four counts the model declares, in the model's own member order,
/// and the account's throttling limits appended last.
///
/// Member set, order and requiredness from
/// `smithy_query(query="shape", shape="com.amazonaws.autoscaling#DescribeAccountLimitsAnswer")`,
/// plus `ThrottlingLimits` — see this module's own documentation for why the schema
/// and the serializer are written out rather than derived.
#[derive(Debug, Clone, Default)]
pub struct DescribeAccountLimitsOutput {
    pub max_number_of_auto_scaling_groups: Option<i32>,
    pub max_number_of_launch_configurations: Option<i32>,
    pub number_of_auto_scaling_groups: Option<i32>,
    pub number_of_launch_configurations: Option<i32>,
    pub throttling_limits: Option<ThrottlingLimits>,
}

#[allow(non_upper_case_globals)]
impl DescribeAccountLimitsOutput {
    pub const M_MAX_NUMBER_OF_AUTO_SCALING_GROUPS: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.autoscaling#DescribeAccountLimitsAnswer$MaxNumberOfAutoScalingGroups",
                "com.amazonaws.autoscaling",
                "MaxNumberOfAutoScalingGroups",
            ),
            ::aws_smithy_schema::ShapeType::Integer,
            "MaxNumberOfAutoScalingGroups",
            0,
        );

    pub const M_MAX_NUMBER_OF_LAUNCH_CONFIGURATIONS: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.autoscaling#DescribeAccountLimitsAnswer$MaxNumberOfLaunchConfigurations",
                "com.amazonaws.autoscaling",
                "MaxNumberOfLaunchConfigurations",
            ),
            ::aws_smithy_schema::ShapeType::Integer,
            "MaxNumberOfLaunchConfigurations",
            1,
        );

    pub const M_NUMBER_OF_AUTO_SCALING_GROUPS: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.autoscaling#DescribeAccountLimitsAnswer$NumberOfAutoScalingGroups",
                "com.amazonaws.autoscaling",
                "NumberOfAutoScalingGroups",
            ),
            ::aws_smithy_schema::ShapeType::Integer,
            "NumberOfAutoScalingGroups",
            2,
        );

    pub const M_NUMBER_OF_LAUNCH_CONFIGURATIONS: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.autoscaling#DescribeAccountLimitsAnswer$NumberOfLaunchConfigurations",
                "com.amazonaws.autoscaling",
                "NumberOfLaunchConfigurations",
            ),
            ::aws_smithy_schema::ShapeType::Integer,
            "NumberOfLaunchConfigurations",
            3,
        );

    pub const M_THROTTLING_LIMITS: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.autoscaling#DescribeAccountLimitsAnswer$ThrottlingLimits",
                "com.amazonaws.autoscaling",
                "ThrottlingLimits",
            ),
            ::aws_smithy_schema::ShapeType::Structure,
            "ThrottlingLimits",
            4,
        );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.autoscaling#DescribeAccountLimitsAnswer",
            "com.amazonaws.autoscaling",
            "DescribeAccountLimitsAnswer",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[
            &Self::M_MAX_NUMBER_OF_AUTO_SCALING_GROUPS,
            &Self::M_MAX_NUMBER_OF_LAUNCH_CONFIGURATIONS,
            &Self::M_NUMBER_OF_AUTO_SCALING_GROUPS,
            &Self::M_NUMBER_OF_LAUNCH_CONFIGURATIONS,
            &Self::M_THROTTLING_LIMITS,
        ],
    );
}

pub static DESCRIBE_ACCOUNT_LIMITS_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &DescribeAccountLimitsOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for DescribeAccountLimitsOutput {
    fn serialize_members(
        &self,
        s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        if let ::std::option::Option::Some(value) = self.max_number_of_auto_scaling_groups {
            s.write_integer(&Self::M_MAX_NUMBER_OF_AUTO_SCALING_GROUPS, value)?;
        }
        if let ::std::option::Option::Some(value) = self.max_number_of_launch_configurations {
            s.write_integer(&Self::M_MAX_NUMBER_OF_LAUNCH_CONFIGURATIONS, value)?;
        }
        if let ::std::option::Option::Some(value) = self.number_of_auto_scaling_groups {
            s.write_integer(&Self::M_NUMBER_OF_AUTO_SCALING_GROUPS, value)?;
        }
        if let ::std::option::Option::Some(value) = self.number_of_launch_configurations {
            s.write_integer(&Self::M_NUMBER_OF_LAUNCH_CONFIGURATIONS, value)?;
        }
        if let ::std::option::Option::Some(limits) = self.throttling_limits.as_ref() {
            s.write_struct(&Self::M_THROTTLING_LIMITS, limits)?;
        }
        Ok(())
    }
}
