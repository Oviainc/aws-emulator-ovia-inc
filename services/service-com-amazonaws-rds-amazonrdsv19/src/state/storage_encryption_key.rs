// SPDX-License-Identifier: CC0-1.0

//! The KMS key a resource's storage encryption is reported under.
//!
//! `CreateDBCluster$KmsKeyId` / `CreateDBInstance$KmsKeyId`: "If you don't specify a value
//! for `KmsKeyId`, then Amazon RDS uses your default KMS key. There is a default KMS key for
//! your Amazon Web Services account. Your Amazon Web Services account has a different
//! default KMS key for each Amazon Web Services Region." So a resource created with
//! encryption turned on and no key named is encrypted under THAT key, and the identifier it
//! reports is that key's — not nothing, which would say an encrypted resource is encrypted
//! under no key at all.

use super::*;

/// The Amazon Web Services managed key an encrypted resource of this service is encrypted
/// under when the request names no `KmsKeyId`: the account's default RDS key, named by the
/// alias AWS publishes for it (`aws/rds`, the alias the KMS console and the RDS User Guide's
/// "Encrypting Amazon RDS resources" name it by).
const RDS_DEFAULT_KEY_ALIAS: &str = "alias/aws/rds";

/// The `KmsKeyId` a cluster or instance whose storage is encrypted reports when its request
/// named no key: the account's default RDS key ([`RDS_DEFAULT_KEY_ALIAS`]), resolved to the
/// key's own ARN through the ONE key resolution this crate has
/// ([`managed_master_secret::resolve_key_arn`]) — so the key the managed master user secret
/// reports and the key a cluster's storage reports are resolved, and fall back, the same way.
pub async fn default_storage_encryption_key_id(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> String {
    managed_master_secret::resolve_key_arn(ctx, region, RDS_DEFAULT_KEY_ALIAS).await
}

/// The `KmsKeyId` to STORE for a resource created with `requested` and `storage_encrypted`.
///
/// The key the request named stands as the caller gave it; a resource encrypted without one
/// named carries the account's default key ([`default_storage_encryption_key_id`]); and a
/// resource whose storage is not encrypted carries no key, because there is none encrypting
/// it.
pub async fn storage_encryption_key_id(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    requested: Option<String>,
    storage_encrypted: bool,
) -> Option<String> {
    match requested {
        Some(named) => Some(named),
        None if storage_encrypted => Some(default_storage_encryption_key_id(ctx, region).await),
        None => None,
    }
}
