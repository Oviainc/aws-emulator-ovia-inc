// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_supported_resource_types` operation.
//!
//! HAND-AUTHORED, unlike its siblings. The scaffold's op-stub generator declined
//! `GetSupportedResourceTypes` (`src/ops/SKIPPED_STUBS.md`) because the
//! operation's input is the Smithy `Unit` shape, which it does not type yet, so
//! neither this module nor the operation module was emitted.
//!
//! The OUTPUT below is generated from the model shape the ordinary way. The
//! INPUT is `smithy.api#Unit` — a structure with no members, which is not a
//! shape the model declares on its own — so its schema and its two serde impls
//! are written out by hand, exactly as the scaffold writes a `Unit` OUTPUT in
//! `ops/update_region_settings.rs`.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json");

/// `GetSupportedResourceTypes`'s input: the Smithy `Unit` shape, so there is
/// nothing on the wire to read and nothing a caller can get wrong.
#[derive(Debug, Clone, Default)]
pub struct GetSupportedResourceTypesInput;

impl GetSupportedResourceTypesInput {
    /// The input shape's schema. `with_http` carries the operation's method and
    /// URI, which is what the REST request path reads to match the route and
    /// fill `@httpLabel` placeholders (there are none here); the status slot is
    /// `None` on an INPUT schema, because the success status is an operation
    /// fact and lives on `Operation::SUCCESS_STATUS`.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    )
    .with_http(::aws_smithy_schema::traits::HttpTrait::new(
        "GET",
        "/supported-resource-types",
        ::std::option::Option::None,
    ));
}

pub static GET_SUPPORTED_RESOURCE_TYPES_INPUT_SCHEMA: &::aws_smithy_schema::Schema =
    &GetSupportedResourceTypesInput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for GetSupportedResourceTypesInput {
    fn serialize_members(
        &self,
        _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for GetSupportedResourceTypesInput {
    fn deserialize(
        __d: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        // The same walk the generated impl of a member-less input runs: read the
        // struct and ignore every member the protocol offers, because the shape
        // declares none.
        __d.read_struct(&Self::SCHEMA, &mut |_member, _d| Ok(()))?;
        Ok(Self)
    }
}

#[smithy_macros::smithy(shape = "com.amazonaws.backup#GetSupportedResourceTypesOutput")]
#[derive(Debug, Clone, Default)]
pub struct GetSupportedResourceTypesOutput {
    pub resource_types: Option<Vec<String>>,
}
