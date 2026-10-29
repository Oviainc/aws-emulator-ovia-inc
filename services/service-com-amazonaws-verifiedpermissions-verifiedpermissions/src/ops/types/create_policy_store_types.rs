// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `create_policy_store` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `CreatePolicyStore`
//! because `CreatePolicyStoreInput$encryptionSettings` targets the
//! `EncryptionSettings` union, one arm of which is `smithy.api#Unit` — a shape the
//! generator cannot type (`src/ops/SKIPPED_STUBS.md`). That union is written out
//! below, exactly as the derive writes a union, so the input schema this service
//! serves carries EVERY modelled `CreatePolicyStoreInput` member, that one
//! included.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.verifiedpermissions#CreatePolicyStoreInput")]
#[derive(Debug, Clone, Default)]
pub struct CreatePolicyStoreInput {
    pub client_token: Option<String>,
    pub validation_settings: ValidationSettings,
    pub description: Option<String>,
    pub deletion_protection: Option<String>,
    pub encryption_settings: Option<EncryptionSettings>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

/// `KmsEncryptionSettings`: "the customer-managed KMS key … to be used for
/// encryption processes", plus the user-defined context to add during them. Only
/// `key` is `@required`, so the context is an `Option` — a request that names a
/// key and no context configures an EMPTY context. Member set, requiredness and
/// the `@pattern` on `key` come from the model, which the derive reads, so the
/// constraint is enforced by the framework's validating walk on this input.
#[smithy_macros::smithy(shape = "com.amazonaws.verifiedpermissions#KmsEncryptionSettings")]
#[derive(Debug, Clone, Default)]
pub struct KmsEncryptionSettings {
    pub key: String,
    pub encryption_context: Option<std::collections::HashMap<String, String>>,
}

/// `EncryptionSettings`: which key a policy store and its child resources are to
/// be encrypted with. The request-side counterpart of
/// [`EncryptionState`](super::get_policy_store_types::EncryptionState), with the
/// same two arms.
///
/// A Smithy union is one key on the wire, so this is written exactly as the
/// derive writes a union — a member schema per arm, in the model's order, and a
/// codec that carries exactly the active arm. It is hand-written for the one
/// reason the module note gives: the `default` arm targets `smithy.api#Unit`,
/// which the vendored codegen cannot type.
#[derive(Debug, Clone)]
pub enum EncryptionSettings {
    /// `EncryptionSettings$kmsEncryptionSettings`: "The KMS encryption settings
    /// for this policy store to encrypt data with. It will contain the
    /// customer-managed KMS key, and a user-defined encryption context."
    KmsEncryptionSettings(KmsEncryptionSettings),
    /// `EncryptionSettings$default`: "This is the default encryption setting. The
    /// policy store uses an Amazon Web Services owned key for encrypting data."
    ///
    /// The arm's target is `smithy.api#Unit`, a structure with no members, so it
    /// carries no value here and is read off the wire as the empty object
    /// [`UnitStructure`] denotes.
    Default,
}

#[allow(non_upper_case_globals)]
impl EncryptionSettings {
    const M_KMS_ENCRYPTION_SETTINGS: ::aws_smithy_schema::Schema = vp_member_schema!(
        "com.amazonaws.verifiedpermissions",
        "EncryptionSettings",
        "kmsEncryptionSettings",
        0,
        struct_of(KmsEncryptionSettings)
    );

    const M_DEFAULT: ::aws_smithy_schema::Schema = vp_member_schema!(
        "com.amazonaws.verifiedpermissions",
        "EncryptionSettings",
        "default",
        1,
        struct_of(UnitStructure)
    );

    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.verifiedpermissions#EncryptionSettings",
            "com.amazonaws.verifiedpermissions",
            "EncryptionSettings",
        ),
        ::aws_smithy_schema::ShapeType::Union,
        &[&Self::M_KMS_ENCRYPTION_SETTINGS, &Self::M_DEFAULT],
    );
}

pub static ENCRYPTION_SETTINGS_SCHEMA: &::aws_smithy_schema::Schema = &EncryptionSettings::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for EncryptionSettings {
    fn serialize_members(
        &self,
        serializer: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        match self {
            Self::KmsEncryptionSettings(value) => {
                serializer.write_struct(&Self::M_KMS_ENCRYPTION_SETTINGS, value)?;
            }
            Self::Default => {
                serializer.write_struct(&Self::M_DEFAULT, &UnitStructure)?;
            }
        }
        Ok(())
    }
}

impl ::aws_smithy_schema_server::DeserializableStruct for EncryptionSettings {
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
                            "union 'EncryptionSettings' had more than one member set",
                        ),
                    );
                }
                ::std::option::Option::Some(0) => {
                    // Naming this arm as the error passes back out is what makes
                    // a required member absent from the arm's structure report
                    // its trail from the request body inwards, in the same form
                    // the validating walk reports a broken constraint on the
                    // same member.
                    let value =
                        <KmsEncryptionSettings as ::aws_smithy_schema_server::DeserializableStruct>
                            ::deserialize(deserializer)
                            .map_err(|error| {
                                ::aws_smithy_schema_server::within_member(
                                    error,
                                    "kmsEncryptionSettings",
                                )
                            })?;
                    out = ::std::option::Option::Some(Self::KmsEncryptionSettings(value));
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
                "union 'EncryptionSettings' had no member set",
            )
        })
    }
}

/// `createdDate` and `lastUpdatedDate` target `TimestampFormat`, which carries
/// `@timestampFormat("date-time")`, so this shape's schema comes from
/// `vp_schema_and_codec!` rather than `#[smithy_macros::smithy(shape = ...)]`:
/// see `types/date_time_wire.rs` for why. Member set, order and requiredness from
/// `smithy_query(query="shape", shape="CreatePolicyStoreOutput")`.
#[derive(Debug, Clone)]
pub struct CreatePolicyStoreOutput {
    pub policy_store_id: String,
    pub arn: String,
    pub created_date: ::aws_smithy_types::DateTime,
    pub last_updated_date: ::aws_smithy_types::DateTime,
}

vp_schema_and_codec!(
    CreatePolicyStoreOutput,
    CREATE_POLICY_STORE_OUTPUT_SCHEMA,
    "com.amazonaws.verifiedpermissions#CreatePolicyStoreOutput",
    "com.amazonaws.verifiedpermissions",
    "CreatePolicyStoreOutput",
    (M_POLICY_STORE_ID, policy_store_id, "policyStoreId", 0, req, string),
    (M_ARN, arn, "arn", 1, req, string),
    (M_CREATED_DATE, created_date, "createdDate", 2, req, date_time),
    (M_LAST_UPDATED_DATE, last_updated_date, "lastUpdatedDate", 3, req, date_time),
);
