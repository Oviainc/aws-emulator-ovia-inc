// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `create_policy_store` operation.
//!
//! Hand-authored: the scaffold op-stub generator declined `CreatePolicyStore`
//! because `CreatePolicyStoreInput$encryptionSettings` targets the
//! `EncryptionSettings` union, one arm of which is `smithy.api#Unit` — a shape the
//! generator cannot type (`src/ops/SKIPPED_STUBS.md`). The member is declared
//! `unsupported` here, so the input schema this service serves carries every other
//! `CreatePolicyStoreInput` member; customer-managed-key encryption is its own
//! slice and no part of the policy-store/policy/authorization loop.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-verifiedpermissions-verifiedpermissions.json");

#[smithy_macros::smithy(
    shape = "com.amazonaws.verifiedpermissions#CreatePolicyStoreInput",
    unsupported = ["encryptionSettings"]
)]
#[derive(Debug, Clone, Default)]
pub struct CreatePolicyStoreInput {
    pub client_token: Option<String>,
    pub validation_settings: ValidationSettings,
    pub description: Option<String>,
    pub deletion_protection: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.verifiedpermissions#CreatePolicyStoreOutput")]
#[derive(Debug, Clone)]
pub struct CreatePolicyStoreOutput {
    pub policy_store_id: String,
    pub arn: String,
    pub created_date: ::aws_smithy_types::DateTime,
    pub last_updated_date: ::aws_smithy_types::DateTime,
}
