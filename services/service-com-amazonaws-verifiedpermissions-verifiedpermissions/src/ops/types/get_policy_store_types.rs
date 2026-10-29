// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_policy_store` operation.
//!
//! Hand-authored, for TWO reasons the vendored codegen cannot cover:
//!
//! 1. `GetPolicyStoreOutput$encryptionState` targets the `EncryptionState` union,
//!    one arm of which targets `smithy.api#Unit` — a shape this service's model
//!    file does not define, so `vendor/smithy-macros`'s member classifier cannot
//!    type it and the scaffold generator declined the whole operation
//!    (`src/ops/SKIPPED_STUBS.md`). The union is written out below instead, with
//!    both of its modelled arms; the `default` arm writes the empty structure
//!    `smithy.api#Unit` denotes — [`UnitStructure`], shared with the request-side
//!    `EncryptionSettings` union — which is what puts `{"default":{}}` on the wire.
//! 2. `createdDate`/`lastUpdatedDate` carry `@timestampFormat("date-time")`, which
//!    the derive does not put on a member schema, so this shape's schema and codec
//!    come from [`vp_schema_and_codec!`] — see `types/date_time_wire.rs` for that
//!    whole story.
//!
//! The member list below is the model's, member for member: same shape ids, same
//! wire names, same order, same shape types. Constraint traits are deliberately
//! not reproduced: the framework's validating walk runs against
//! `Operation::INPUT_SCHEMA` only, so nothing reads them on an output shape.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.verifiedpermissions#GetPolicyStoreInput")]
#[derive(Debug, Clone, Default)]
pub struct GetPolicyStoreInput {
    pub policy_store_id: String,
    pub tags: bool,
}

/// `KmsEncryptionState`: the customer-managed KMS key a policy store is
/// encrypted with, and the user-defined context added during encryption. Both
/// members are `@required`, so neither is an `Option`.
#[smithy_macros::smithy(shape = "com.amazonaws.verifiedpermissions#KmsEncryptionState")]
#[derive(Debug, Clone, Default)]
pub struct KmsEncryptionState {
    pub key: String,
    pub encryption_context: std::collections::HashMap<String, String>,
}

/// `EncryptionState`: how a policy store and its child resources are encrypted.
///
/// A Smithy union is one key on the wire, so this is written exactly as the
/// derive writes a union — a member schema per arm, in the model's order, and a
/// codec that carries exactly the active arm.
#[derive(Debug, Clone)]
pub enum EncryptionState {
    /// `EncryptionState$kmsEncryptionState`: "The KMS encryption settings
    /// currently configured for this policy store to encrypt data with."
    KmsEncryptionState(KmsEncryptionState),
    /// `EncryptionState$default`: "This is the default encryption state. The
    /// policy store is encrypted using an Amazon Web Services owned key."
    ///
    /// The arm's target is `smithy.api#Unit`, a structure with no members, so it
    /// carries no value here and goes on the wire as the empty object
    /// [`UnitStructure`] writes — the one such structure this service has, in
    /// `types/shared_types.rs`, because the request-side `EncryptionSettings`
    /// union carries the same arm.
    Default,
}

#[allow(non_upper_case_globals)]
impl EncryptionState {
    const M_KMS_ENCRYPTION_STATE: ::aws_smithy_schema::Schema =
        vp_member_schema!(
            "com.amazonaws.verifiedpermissions",
            "EncryptionState",
            "kmsEncryptionState",
            0,
            struct_of(KmsEncryptionState)
        );

    const M_DEFAULT: ::aws_smithy_schema::Schema = vp_member_schema!(
        "com.amazonaws.verifiedpermissions",
        "EncryptionState",
        "default",
        1,
        struct_of(UnitStructure)
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.verifiedpermissions#EncryptionState",
            "com.amazonaws.verifiedpermissions",
            "EncryptionState",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[&Self::M_KMS_ENCRYPTION_STATE, &Self::M_DEFAULT],
    );
}

pub static ENCRYPTION_STATE_SCHEMA: &::aws_smithy_schema::Schema = &EncryptionState::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for EncryptionState {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::KmsEncryptionState(value) => {
                serializer.write_struct(&Self::M_KMS_ENCRYPTION_STATE, value)?;
            }
            Self::Default => {
                serializer.write_struct(&Self::M_DEFAULT, &UnitStructure)?;
            }
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for EncryptionState {
    fn deserialize(
        deserializer: &mut dyn ::aws_smithy_schema::serde::ShapeDeserializer,
    ) -> ::std::result::Result<Self, ::aws_smithy_schema::serde::SerdeError> {
        let mut out: ::std::option::Option<Self> = ::std::option::Option::None;
        deserializer.read_struct(&Self::SCHEMA, &mut |member, deserializer| {
            match member.member_index() {
                // A union carries EXACTLY one member, so a second one on the
                // wire is ambiguous and is refused rather than overwritten.
                ::std::option::Option::Some(0) | ::std::option::Option::Some(1)
                    if out.is_some() =>
                {
                    return ::std::result::Result::Err(
                        ::aws_smithy_schema::serde::SerdeError::custom(
                            "union 'EncryptionState' had more than one member set",
                        ),
                    );
                }
                ::std::option::Option::Some(0) => {
                    out = ::std::option::Option::Some(Self::KmsEncryptionState(
                        <KmsEncryptionState as ::aws_smithy_schema_server::DeserializableStruct>
                            ::deserialize(deserializer)?,
                    ));
                }
                ::std::option::Option::Some(1) => {
                    <UnitStructure as ::aws_smithy_schema_server::DeserializableStruct>
                        ::deserialize(deserializer)?;
                    out = ::std::option::Option::Some(Self::Default);
                }
                _ => {}
            }
            Ok(())
        })?;
        out.ok_or_else(|| {
            ::aws_smithy_schema::serde::SerdeError::custom(
                "union 'EncryptionState' had no member set",
            )
        })
    }
}

/// `GetPolicyStoreOutput`. Its schema and codec come from
/// [`vp_schema_and_codec!`] rather than `#[smithy_macros::smithy(shape = ...)]`
/// for the two reasons in the module note; member set, order and requiredness
/// from `smithy_query(query="shape", shape="GetPolicyStoreOutput")`.
#[derive(Debug, Clone)]
pub struct GetPolicyStoreOutput {
    pub policy_store_id: String,
    pub arn: String,
    pub validation_settings: ValidationSettings,
    pub created_date: ::aws_smithy_types::DateTime,
    pub last_updated_date: ::aws_smithy_types::DateTime,
    pub description: Option<String>,
    pub deletion_protection: Option<String>,
    pub encryption_state: Option<EncryptionState>,
    pub cedar_version: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

vp_schema_and_codec!(
    GetPolicyStoreOutput,
    GET_POLICY_STORE_OUTPUT_SCHEMA,
    "com.amazonaws.verifiedpermissions#GetPolicyStoreOutput",
    "com.amazonaws.verifiedpermissions",
    "GetPolicyStoreOutput",
    (M_POLICY_STORE_ID, policy_store_id, "policyStoreId", 0, req, string),
    (M_ARN, arn, "arn", 1, req, string),
    (
        M_VALIDATION_SETTINGS,
        validation_settings,
        "validationSettings",
        2,
        req,
        struct_of(ValidationSettings)
    ),
    (M_CREATED_DATE, created_date, "createdDate", 3, req, date_time),
    (M_LAST_UPDATED_DATE, last_updated_date, "lastUpdatedDate", 4, req, date_time),
    (M_DESCRIPTION, description, "description", 5, opt, string),
    (M_DELETION_PROTECTION, deletion_protection, "deletionProtection", 6, opt, string),
    (
        M_ENCRYPTION_STATE,
        encryption_state,
        "encryptionState",
        7,
        opt,
        union_of(EncryptionState)
    ),
    (M_CEDAR_VERSION, cedar_version, "cedarVersion", 8, opt, string),
    (M_TAGS, tags, "tags", 9, opt, map_of_string),
);
