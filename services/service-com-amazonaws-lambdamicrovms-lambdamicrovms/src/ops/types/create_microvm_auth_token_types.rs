// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `create_microvm_auth_token` operation.
//!
//! HAND-AUTHORED, not generated: the scaffold op-stub generator declined this
//! operation (`ops/SKIPPED_STUBS.md`: "shape missing from model: smithy.api#Unit")
//! because `PortSpecification$allPorts` targets the Smithy prelude's `Unit`, which the
//! on-disk model does not carry as a shape — so the `union_for` codegen cannot type
//! that member. [`PortSpecification`] and [`AllPorts`] therefore carry written-out
//! schemas and serde impls, exactly as the vendored union codegen would emit them
//! (member order, member indices and the one-member-set arity check all match);
//! everything else here is derived from the model by the macro.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-lambdamicrovms-lambdamicrovms.json");

/// `PortSpecification$port`'s constraint traits, from `PortNumber`:
/// `@range(min: 1, max: 65535)` (`smithy_query(query="shape",
/// shape="PortSpecification")`).
#[allow(non_upper_case_globals)]
static PORT_SPECIFICATION_M_PORT_TRAITS: ::std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    ::std::sync::LazyLock::new(|| {
        let mut m = ::aws_smithy_schema::TraitMap::new();
        m.insert(::std::boxed::Box::new(
            ::aws_smithy_schema_server::constraints::RangeTrait::new(
                ::std::option::Option::Some(1.0),
                ::std::option::Option::Some(65535.0),
            ),
        ));
        m
    });

/// The `smithy.api#Unit` a `PortSpecification$allPorts` member carries: "every port",
/// which is a choice with no value of its own.
///
/// The prelude shape is absent from the on-disk model, so this stands in for it with
/// the shape id the model's member targets — the same way `TagResource`'s `Unit`
/// output is written out in `ops/tag_resource.rs`.
#[derive(Debug, Clone, Default)]
pub struct AllPorts;

impl AllPorts {
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

impl ::aws_smithy_schema::serde::SerializableStruct for AllPorts {
    fn serialize_members(
        &self,
        _serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for AllPorts {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        // The unit carries no member; the walk is still performed so a member the
        // caller sent is consumed rather than left for the enclosing shape.
        deserializer.read_struct(&Self::SCHEMA, &mut |_member, _d| Ok(()))?;
        Ok(Self)
    }
}

/// A contiguous port range, `com.amazonaws.lambdamicrovms#PortRange`.
#[smithy_macros::smithy(shape = "com.amazonaws.lambdamicrovms#PortRange")]
#[derive(Debug, Clone, Default)]
pub struct PortRange {
    pub start_port: i32,
    pub end_port: i32,
}

/// One entry of `CreateMicrovmAuthTokenRequest$allowedPorts`: a single port, a range
/// of ports, or every port.
#[derive(Debug, Clone)]
pub enum PortSpecification {
    Port(i32),
    Range(PortRange),
    AllPorts(AllPorts),
}

#[allow(non_upper_case_globals)]
impl PortSpecification {
    const NAMESPACE: &'static str = "com.amazonaws.lambdamicrovms";

    pub const M_PORT: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms#PortSpecification$port",
            Self::NAMESPACE,
            "port",
        ),
        ::aws_smithy_schema::ShapeType::Integer,
        "port",
        0,
    )
    .with_traits(&PORT_SPECIFICATION_M_PORT_TRAITS);

    pub const M_RANGE: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms#PortSpecification$range",
            Self::NAMESPACE,
            "range",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        "range",
        1,
    );

    pub const M_ALL_PORTS: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms#PortSpecification$allPorts",
            Self::NAMESPACE,
            "allPorts",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        "allPorts",
        2,
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.lambdamicrovms#PortSpecification",
            Self::NAMESPACE,
            "PortSpecification",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[&Self::M_PORT, &Self::M_RANGE, &Self::M_ALL_PORTS],
    );
}

pub static PORT_SPECIFICATION_SCHEMA: &::aws_smithy_schema::Schema = &PortSpecification::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for PortSpecification {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::Port(port) => serializer.write_integer(&Self::M_PORT, *port)?,
            Self::Range(range) => serializer.write_struct(&Self::M_RANGE, range)?,
            Self::AllPorts(all_ports) => {
                serializer.write_struct(&Self::M_ALL_PORTS, all_ports)?
            }
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for PortSpecification {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut out: ::std::option::Option<Self> = ::std::option::Option::None;
        deserializer.read_struct(&Self::SCHEMA, &mut |member, d| {
            let read = match member.member_index() {
                Some(0) => Some(Self::Port(d.read_integer(&Self::M_PORT)?)),
                Some(1) => Some(Self::Range(
                    <PortRange as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(
                        d,
                    )?,
                )),
                Some(2) => Some(Self::AllPorts(
                    <AllPorts as ::aws_smithy_schema_server::DeserializableStruct>::deserialize(d)?,
                )),
                _ => None,
            };
            if let Some(value) = read {
                // A union carries EXACTLY one member, so a second member on the wire
                // is ambiguous and must be refused rather than silently overwritten.
                if out.is_some() {
                    return ::std::result::Result::Err(
                        ::aws_smithy_schema::serde::SerdeError::custom(
                            "union 'PortSpecification' had more than one member set",
                        ),
                    );
                }
                out = Some(value);
            }
            Ok(())
        })?;
        out.ok_or_else(|| {
            ::aws_smithy_schema::serde::SerdeError::custom(
                "union 'PortSpecification' had no member set",
            )
        })
    }
}

#[smithy_macros::smithy(shape = "com.amazonaws.lambdamicrovms#CreateMicrovmAuthTokenRequest")]
#[derive(Debug, Clone, Default)]
pub struct CreateMicrovmAuthTokenInput {
    pub microvm_identifier: String,
    pub expiration_in_minutes: i32,
    pub allowed_ports: Vec<PortSpecification>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.lambdamicrovms#CreateMicrovmAuthTokenResponse")]
#[derive(Debug, Clone, Default)]
pub struct CreateMicrovmAuthTokenOutput {
    pub auth_token: std::collections::HashMap<String, String>,
}
