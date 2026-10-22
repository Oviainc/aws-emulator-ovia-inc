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

#[smithy_macros::smithy(
    shape = "com.amazonaws.verifiedpermissions#GetPolicyStoreOutput",
    unsupported = ["encryptionState"]
)]
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
