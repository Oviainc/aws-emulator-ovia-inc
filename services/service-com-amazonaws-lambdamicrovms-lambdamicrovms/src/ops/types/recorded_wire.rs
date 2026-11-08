// SPDX-License-Identifier: CC0-1.0

//! The members this service's RECORDED responses carry beside the modelled ones, and
//! the wrapper that writes them.
//!
//! `#[smithy_macros::smithy(shape = …)]` derives a structure's schema, serializer and
//! deserializer from the model's member list and admits exactly that list, so a Rust
//! field with no Smithy member behind it is a compile error. A member the recorded
//! service answers with but the model does not declare therefore cannot live on a
//! derived structure at all: it is written AFTER the modelled payload's own members,
//! under a member schema declared here, by [`RecordedMembers`].
//!
//! Two such members exist on this service's successful bodies:
//!
//! * `id` on every response that reports a stored MicroVM IMAGE record — the image's
//!   own identity, which a client keys its records on and joins a later read or listing
//!   back to the create by. `CreateMicrovmImageResponse`, `GetMicrovmImageOutput`,
//!   `UpdateMicrovmImageResponse` and `MicrovmImageSummary` declare `imageArn`, `name`,
//!   `state`, `latestActiveImageVersion`, `latestFailedImageVersion`, `createdAt` (and
//!   the configuration members) and no `id`
//!   (`smithy_query(query="shape",shape="CreateMicrovmImageResponse,GetMicrovmImageOutput,MicrovmImageSummary",model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json")`).
//! * `supportedCpuConfigurations` on `ManagedMicrovmImageVersion` — which CPU
//!   architectures that published generation of an AWS-managed base image supports. The
//!   shape declares `imageArn`, `imageVersion`, `status`, `createdAt` and `updatedAt`
//!   alone (same `smithy_query`).
//! * `versionStateTimeBucket` on every response that reports one VERSION of a MicroVM
//!   image — the time bucket that version's state settled in
//!   ([`crate::state::MicrovmImageVersion::reported_state_time_bucket`]), which a client
//!   reading a settled build groups its own records by.
//!   `GetMicrovmImageVersionOutput` and `MicrovmImageVersionSummary` declare the
//!   version's build inputs, `state`, `status`, `stateReason`, `createdAt`, `updatedAt`
//!   and `tags` and no such member
//!   (`smithy_query(query="shape",shape="GetMicrovmImageVersionOutput,MicrovmImageVersionSummary",model_path="services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json")`).
//!
//! Each member schema is declared in the `com.amazonaws.lambdamicrovms.recorded`
//! namespace rather than in the service's own: the member is NOT in the model, so its
//! shape id must neither claim it is nor collide with a member the model may declare
//! later. The member INDEX is the next free one after the modelled members, so a later
//! model refresh that declares the member only has to move it into place and put the
//! derive back.
//!
//! Deliberately absent, for the reasons the derive's own output would be inert here:
//! the constraint traits (the validating walk runs on operation INPUTS only, and these
//! members are output-only) and `DeserializableStruct` (nothing reads these answers
//! back off the wire — `smithy_server_runtime::Operation` requires it of `Input`
//! alone).
#![allow(unused_imports)]

use super::*;

/// The namespace the members this model does not declare are declared in; see this
/// module's own documentation.
const RECORDED_NAMESPACE: &str = "com.amazonaws.lambdamicrovms.recorded";

/// `id` on `CreateMicrovmImageResponse`, whose 21 modelled members occupy indices
/// `0..=20`.
pub const IMAGE_ID_ON_CREATE_RESPONSE: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms.recorded#CreateMicrovmImageResponse$id",
            RECORDED_NAMESPACE,
            "id",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "id",
        21,
    );

/// `id` on `GetMicrovmImageOutput`, whose eight modelled members occupy indices
/// `0..=7`.
pub const IMAGE_ID_ON_GET_OUTPUT: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms.recorded#GetMicrovmImageOutput$id",
            RECORDED_NAMESPACE,
            "id",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "id",
        8,
    );

/// `id` on `UpdateMicrovmImageResponse`, whose 20 modelled members occupy indices
/// `0..=19`.
pub const IMAGE_ID_ON_UPDATE_RESPONSE: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms.recorded#UpdateMicrovmImageResponse$id",
            RECORDED_NAMESPACE,
            "id",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "id",
        20,
    );

/// `id` on `MicrovmImageSummary` — one listing row — whose six modelled members occupy
/// indices `0..=5`.
pub const IMAGE_ID_ON_SUMMARY: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms.recorded#MicrovmImageSummary$id",
            RECORDED_NAMESPACE,
            "id",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "id",
        6,
    );

/// `supportedCpuConfigurations` on `ManagedMicrovmImageVersion`, whose five modelled
/// members occupy indices `0..=4`. A list of `CpuConfiguration`, the modelled structure
/// whose one member is the `architecture`.
pub const SUPPORTED_CPU_CONFIGURATIONS_ON_MANAGED_VERSION: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms.recorded#ManagedMicrovmImageVersion$supportedCpuConfigurations",
            RECORDED_NAMESPACE,
            "supportedCpuConfigurations",
        ),
        ::aws_smithy_schema::ShapeType::List,
        "supportedCpuConfigurations",
        5,
    );

/// `versionStateTimeBucket` on `GetMicrovmImageVersionOutput`, whose 20 modelled members
/// occupy indices `0..=19`.
pub const VERSION_STATE_TIME_BUCKET_ON_GET_VERSION_OUTPUT: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms.recorded#GetMicrovmImageVersionOutput$versionStateTimeBucket",
            RECORDED_NAMESPACE,
            "versionStateTimeBucket",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "versionStateTimeBucket",
        20,
    );

/// `versionStateTimeBucket` on `MicrovmImageVersionSummary` — one row of the version
/// listing — whose 20 modelled members occupy indices `0..=19`.
pub const VERSION_STATE_TIME_BUCKET_ON_VERSION_SUMMARY: ::aws_smithy_schema::Schema =
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms.recorded#MicrovmImageVersionSummary$versionStateTimeBucket",
            RECORDED_NAMESPACE,
            "versionStateTimeBucket",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "versionStateTimeBucket",
        20,
    );

/// One member the RECORDED service carries beside a modelled response, with the member
/// schema it is written under.
#[derive(Debug, Clone)]
pub enum RecordedMember {
    /// A string-valued member.
    Text(&'static ::aws_smithy_schema::Schema, String),
    /// A list of `CpuConfiguration`, each element written against that modelled
    /// structure's own schema — the way the vendored codegen writes a list of
    /// structures.
    CpuConfigurations(&'static ::aws_smithy_schema::Schema, Vec<CpuConfiguration>),
}

/// A modelled response (or one row of one) plus the members the recorded service
/// carries beside it.
///
/// The enclosing schema stays the model's own: a client that knows only the modelled
/// shape reads the payload and ignores the extra keys.
#[derive(Debug, Clone)]
pub struct RecordedMembers<T> {
    /// The modelled payload — the only part an SDK-generated client reads.
    pub payload: T,
    /// The recorded-only members, written in order after the modelled ones.
    pub members: Vec<RecordedMember>,
}

impl<T> RecordedMembers<T> {
    /// A modelled payload carrying the recorded members `members`.
    pub fn new(payload: T, members: Vec<RecordedMember>) -> Self {
        Self { payload, members }
    }
}

impl<T: ::aws_smithy_schema::serde::SerializableStruct>
    ::aws_smithy_schema::serde::SerializableStruct for RecordedMembers<T>
{
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        self.payload.serialize_members(serializer)?;
        for member in &self.members {
            match member {
                RecordedMember::Text(schema, value) => serializer.write_string(schema, value)?,
                RecordedMember::CpuConfigurations(schema, configurations) => {
                    serializer.write_list(schema, &|elements| {
                        for configuration in configurations {
                            elements.write_struct(&CpuConfiguration::SCHEMA, configuration)?;
                        }
                        Ok(())
                    })?;
                }
            }
        }
        Ok(())
    }
}
