// SPDX-License-Identifier: CC0-1.0

//! The `KmsKeyId` an encrypted ElastiCache resource is stored under.
//!
//! Every member of this API that names a customer managed key — a replication
//! group's `KmsKeyId`, a snapshot's, a serverless cache's — accepts any form KMS
//! accepts: a key id, a key ARN, an alias name (`alias/<name>`) or an alias ARN.
//! What the resource REPORTS afterwards is the key itself, by ARN: an alias is
//! re-targetable, so a client auditing which key encrypts a resource (or matching
//! it against what `kms DescribeKey` answers) needs the key's own identity rather
//! than a name that may since point elsewhere.
//!
//! So the identifier is resolved ONCE, on the call that stores it, and the
//! resolved ARN is what is persisted and read back. KMS is reached the way the
//! rest of this emulator reaches another service: the AWS SDK driven over the
//! emulator's own in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so no socket is
//! opened and this crate names no other service's crate.
//!
//! A lookup KMS does not answer — a deployment with no KMS mounted beside this
//! service, a server fault, a key this account does not hold — says nothing about
//! the identifier, so the request's own value is stored unchanged: none of the
//! operations that take a `KmsKeyId` models a KMS fault, so a key that cannot be
//! resolved is not a refusal this service may answer with.

/// The `KmsKeyId` to STORE for the identifier `requested` names: the ARN of the
/// key it resolves to, or `requested` itself when KMS does not resolve it.
///
/// `None` in, `None` out — a request that names no key leaves the resource
/// encrypted under the service-managed key, which carries no identifier.
pub async fn stored_key_id(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    requested: Option<String>,
) -> Option<String> {
    let requested = requested?;
    match resolve_key_arn(ctx, region, &requested).await {
        Some(arn) => Some(arn),
        None => Some(requested),
    }
}

/// The ARN of the key `key_id` identifies, or `None` when KMS did not answer
/// with one.
async fn resolve_key_arn(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
) -> Option<String> {
    match kms_client(ctx, region)
        .describe_key()
        .key_id(key_id)
        .send()
        .await
    {
        Ok(described) => described
            .key_metadata()
            .and_then(|metadata| metadata.arn())
            .map(str::to_string),
        Err(error) => {
            ::tracing::debug!(
                kms_key = key_id,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "KMS did not resolve the key this request names; storing the identifier as sent"
            );
            None
        }
    }
}

/// A KMS client over the in-process callback into this same emulator.
///
/// `region` is the request's own signing region, so the key is resolved where the
/// resource that names it lives. The transport is in-process, so there is nothing
/// to retry, time out or watch for a stalled stream — and each of those would need
/// a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly.
fn kms_client(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_kms::Client {
    let config = ::aws_sdk_kms::Config::builder()
        .behavior_version(::aws_sdk_kms::config::BehaviorVersion::latest())
        .region(::aws_sdk_kms::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://kms.{region}.amazonaws.com"))
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_kms::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_kms::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_kms::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}
