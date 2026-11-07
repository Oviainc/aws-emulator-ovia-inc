// SPDX-License-Identifier: CC0-1.0

//! The projections this service REPORTS where its response carries a member the published
//! shape does not bind.
//!
//! Each type here wraps the modelled shape a handler built from state and writes it out with
//! that one extra member. They are hand-declared rather than generated from the shape by
//! `#[smithy_macros::smithy(shape = ...)]` — the generator writes exactly the members the
//! model binds — for the same reason
//! [`super::update_data_quality_ruleset_types::UpdateDataQualityRulesetOutput`] is: the
//! service answers with more than the published shape names. The shape ids and the member
//! names below are the wire ones, and every published member keeps the target the model gives
//! it.
//!
//! Each wrapper BORROWS the modelled value, so a handler keeps building its answer from state
//! exactly as before and nothing is copied on the way out. Only the SERIALIZE direction is
//! implemented: these shapes are written by this server and never read back by it, so there
//! is no `DeserializableStruct` here.

#![allow(unused_imports)]

use super::*;

/// Whether a security configuration enforces SSL on the JDBC connections Glue makes under
/// it, as the service reports it inside every `EncryptionConfiguration` it answers with.
///
/// RECORDED, not modelled: the published `EncryptionConfiguration` binds `S3Encryption`,
/// `CloudWatchEncryption`, `JobBookmarksEncryption` and `DataQualityEncryption` only, so no
/// `CreateSecurityConfiguration` can state this member and no configuration this service
/// stores enforces JDBC SSL for any connection. The service reports the flag regardless, so
/// a client auditing JDBC SSL enforcement reads an explicit `false` rather than nothing at
/// all.
const ENFORCE_JDBC_SSL: bool = false;

/// The state of ENCRYPTED CATALOG SEARCH the Data Catalog reports beside its encryption
/// settings.
///
/// RECORDED, not modelled: the published `DataCatalogEncryptionSettings` binds
/// `EncryptionAtRest` and `ConnectionPasswordEncryption` only, so no
/// `PutDataCatalogEncryptionSettings` can turn encrypted catalog search on and no catalog
/// this service holds has it on. The service reports the status regardless, so a client
/// checking catalog-search encryption reads the state the catalog is in.
const ENCRYPTED_CATALOG_SEARCH_NOT_ENABLED: &str = "NOT_ENABLED";

/// `EncryptionConfiguration` as this service reports one: the configuration's published
/// members, as the stored configuration carries them, plus [`ENFORCE_JDBC_SSL`].
///
/// The published members are written by the wrapped value's OWN serializer, under their own
/// member schemas, so this type restates none of them and cannot drift from the model.
pub struct ReportedEncryptionConfiguration<'a>(pub &'a EncryptionConfiguration);

#[allow(non_upper_case_globals)]
impl ReportedEncryptionConfiguration<'_> {
    /// The member index continues after the four the published shape binds.
    const M_ENFORCE_JDBC_SSL: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.glue#EncryptionConfiguration$EnforceJdbcSsl",
                "com.amazonaws.glue",
                "EnforceJdbcSsl",
            ),
            ::aws_smithy_schema::ShapeType::Boolean,
            "EnforceJdbcSsl",
            4,
        );
}

impl ::aws_smithy_schema::serde::SerializableStruct for ReportedEncryptionConfiguration<'_> {
    fn serialize_members(
        &self,
        __s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        self.0.serialize_members(__s)?;
        __s.write_boolean(&Self::M_ENFORCE_JDBC_SSL, ENFORCE_JDBC_SSL)?;
        Ok(())
    }
}

/// `SecurityConfiguration` as this service reports one.
///
/// Every member is the published one with the target the model gives it (`Name`:
/// `NameString`, `CreatedTimeStamp`: `TimestampValue`, `EncryptionConfiguration`:
/// `EncryptionConfiguration`); what this type adds is the NESTING, because the encryption
/// configuration inside it is written through [`ReportedEncryptionConfiguration`].
pub struct ReportedSecurityConfiguration<'a>(pub &'a SecurityConfiguration);

#[allow(non_upper_case_globals)]
impl ReportedSecurityConfiguration<'_> {
    const M_NAME: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.glue#SecurityConfiguration$Name",
            "com.amazonaws.glue",
            "Name",
        ),
        ::aws_smithy_schema::ShapeType::String,
        "Name",
        0,
    );

    const M_CREATED_TIME_STAMP: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.glue#SecurityConfiguration$CreatedTimeStamp",
                "com.amazonaws.glue",
                "CreatedTimeStamp",
            ),
            ::aws_smithy_schema::ShapeType::Timestamp,
            "CreatedTimeStamp",
            1,
        );

    const M_ENCRYPTION_CONFIGURATION: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.glue#SecurityConfiguration$EncryptionConfiguration",
                "com.amazonaws.glue",
                "EncryptionConfiguration",
            ),
            ::aws_smithy_schema::ShapeType::Structure,
            "EncryptionConfiguration",
            2,
        );

    /// The shape's own schema, which a listing's element write names.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static(
            "com.amazonaws.glue#SecurityConfiguration",
            "com.amazonaws.glue",
            "SecurityConfiguration",
        ),
        ::aws_smithy_schema::ShapeType::Structure,
        &[
            &Self::M_NAME,
            &Self::M_CREATED_TIME_STAMP,
            &Self::M_ENCRYPTION_CONFIGURATION,
        ],
    );
}

impl ::aws_smithy_schema::serde::SerializableStruct for ReportedSecurityConfiguration<'_> {
    fn serialize_members(
        &self,
        __s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        // A member the stored configuration carries no value for is omitted, exactly as the
        // generated serializers omit an absent optional member.
        if let Some(__v) = self.0.name.as_ref() {
            __s.write_string(&Self::M_NAME, __v.as_str())?;
        }
        if let Some(__v) = self.0.created_time_stamp.as_ref() {
            __s.write_timestamp(&Self::M_CREATED_TIME_STAMP, __v)?;
        }
        if let Some(__v) = self.0.encryption_configuration.as_ref() {
            __s.write_struct(
                &Self::M_ENCRYPTION_CONFIGURATION,
                &ReportedEncryptionConfiguration(__v),
            )?;
        }
        Ok(())
    }
}

/// `DataCatalogEncryptionSettings` as this service reports them: the settings' published
/// members, as the catalog holds them, plus [`ENCRYPTED_CATALOG_SEARCH_NOT_ENABLED`].
///
/// The published members are written by the wrapped value's OWN serializer, under their own
/// member schemas, so this type restates none of them.
pub struct ReportedDataCatalogEncryptionSettings<'a>(pub &'a DataCatalogEncryptionSettings);

#[allow(non_upper_case_globals)]
impl ReportedDataCatalogEncryptionSettings<'_> {
    /// The member index continues after the two the published shape binds.
    const M_ENCRYPTED_CATALOG_SEARCH_STATUS: ::aws_smithy_schema::Schema =
        ::aws_smithy_schema::Schema::new_member(
            ::aws_smithy_schema::ShapeId::from_static(
                "com.amazonaws.glue#DataCatalogEncryptionSettings$EncryptedCatalogSearchStatus",
                "com.amazonaws.glue",
                "EncryptedCatalogSearchStatus",
            ),
            ::aws_smithy_schema::ShapeType::String,
            "EncryptedCatalogSearchStatus",
            2,
        );
}

impl ::aws_smithy_schema::serde::SerializableStruct
    for ReportedDataCatalogEncryptionSettings<'_>
{
    fn serialize_members(
        &self,
        __s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer,
    ) -> ::std::result::Result<(), ::aws_smithy_schema::serde::SerdeError> {
        self.0.serialize_members(__s)?;
        __s.write_string(
            &Self::M_ENCRYPTED_CATALOG_SEARCH_STATUS,
            ENCRYPTED_CATALOG_SEARCH_NOT_ENABLED,
        )?;
        Ok(())
    }
}
