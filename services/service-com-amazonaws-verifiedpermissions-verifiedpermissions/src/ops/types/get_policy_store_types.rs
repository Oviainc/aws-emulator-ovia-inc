// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_policy_store` operation.
//!
//! Hand-authored for the same reason as `create_policy_store_types`:
//! `GetPolicyStoreOutput$encryptionState` targets the `EncryptionState` union,
//! one arm of which is `smithy.api#Unit`, so the scaffold generator declined the
//! whole operation. The member is declared `unsupported` here; every other
//! `GetPolicyStoreOutput` member is served.
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

/// `createdDate`/`lastUpdatedDate` target `TimestampFormat`
/// (`@timestampFormat("date-time")`), so this shape's schema comes from
/// `vp_schema_and_codec!` rather than `#[smithy_macros::smithy(shape = ...)]` — see
/// `types/date_time_wire.rs`. Member set, order and requiredness from
/// `smithy_query(query="shape", shape="GetPolicyStoreOutput")`; `encryptionState` is the
/// one member this slice does not serve, and — exactly as the attribute's `unsupported`
/// list does — it takes no member index, so `cedarVersion` and `tags` follow
/// `deletionProtection` directly.
#[derive(Debug, Clone)]
pub struct GetPolicyStoreOutput {
    pub policy_store_id: String,
    pub arn: String,
    pub validation_settings: ValidationSettings,
    pub created_date: ::aws_smithy_types::DateTime,
    pub last_updated_date: ::aws_smithy_types::DateTime,
    pub description: Option<String>,
    pub deletion_protection: Option<String>,
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
    (M_CEDAR_VERSION, cedar_version, "cedarVersion", 7, opt, string),
    (M_TAGS, tags, "tags", 8, opt, map_of_string),
);
