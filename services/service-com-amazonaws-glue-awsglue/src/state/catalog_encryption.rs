// SPDX-License-Identifier: CC0-1.0

//! Encryption at rest for the Data Catalog's own objects: the KMS key a database, table
//! or partition was WRITTEN under, and the KMS hop a write and a read of such an object
//! make.
//!
//! `PutDataCatalogEncryptionSettings` sets the security configuration of one catalog, and
//! "after the configuration has been set, the specified encryption is applied to every
//! catalog write thereafter" (the operation's documentation). So the key is a property of
//! each WRITE, not of the catalog's current setting: a later write under another key, or
//! with encryption turned off, does not re-encrypt what is already stored, and the key an
//! object is answered through is the one its own write recorded
//! ([`ObjectEncryption`] on [`super::DatabaseEntry`], [`super::TableEntry`] and
//! [`super::PartitionEntry`]).
//!
//! What that makes a catalog read DEPEND ON is the key:
//!
//! * A write under a keyed mode asks KMS for the object's data key under the configured
//!   key ([`seal`]). Only the form KMS encrypted that data key in is kept, so recovering
//!   it is something only KMS can do.
//! * A read of such an object recovers it through KMS first ([`unseal`]); a KMS refusal
//!   about the key — a key that has been disabled, scheduled for deletion or deleted, or
//!   one this caller may not use — is the catalog read's refusal too, relayed under the
//!   modelled `GlueEncryptionException`.
//!
//! KMS is reached the way this service already reaches another service: the AWS SDK driven
//! over the emulator's OWN in-process transport, through the operation callback
//! ([`smithy_server_runtime::OperationContext::callback`]), so no socket is opened and
//! this crate names no other service's crate.
//!
//! A lookup that KMS does not answer at all — a deployment with no KMS mounted beside this
//! service, a server fault, a throttle — is evidence of NOTHING about the key, so it never
//! refuses a request: the write records no key material and the read answers from what is
//! stored. Only a definite negative FROM KMS refuses.

use ::aws_sdk_kms::error::ProvideErrorMetadata;
use ::aws_smithy_runtime_api::client::orchestrator::HttpResponse;

use super::ComAmazonawsGlueAwsglueState;

/// The `CatalogEncryptionMode` values under which a catalog write is encrypted with a KMS
/// key named by `EncryptionAtRest$SseAwsKmsKeyId`.
///
/// Source: the modelled `CatalogEncryptionMode` enum, whose published values are carried
/// by [`crate::ops::types::CATALOG_ENCRYPTION_MODE_VALUES`] — `DISABLED`, `SSE-KMS` and
/// `SSE-KMS-WITH-SERVICE-ROLE`. `DISABLED` is the one that names no key (see
/// [`super::CATALOG_ENCRYPTION_MODE_DISABLED`]), so the other two are these.
pub const KEYED_ENCRYPTION_MODES: [&str; 2] = ["SSE-KMS", "SSE-KMS-WITH-SERVICE-ROLE"];

/// The key material ONE catalog object was written under.
///
/// The data key is held only as KMS returned it — encrypted under the catalog's key — so
/// no read of the object it belongs to can be answered without KMS opening it again under
/// that key. The identifier is the one the catalog's settings named at the time of the
/// write, which is what a later read asks KMS about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectEncryption {
    /// `EncryptionAtRest$SseAwsKmsKeyId` as the catalog's settings carried it when this
    /// object was written — a key id, a key ARN or an alias, whichever the client set.
    pub key_id: String,
    /// The object's data key, in the encrypted form KMS answered `GenerateDataKey` with.
    pub encrypted_data_key: Vec<u8>,
}

/// The key a write to `catalog_id` is encrypted under: the `SseAwsKmsKeyId` of that
/// catalog's stored `EncryptionAtRest` while its mode is one of
/// [`KEYED_ENCRYPTION_MODES`], and `None` while the catalog encrypts its objects under no
/// key of the caller's.
///
/// Read from the stored record under the store's own guard, so a caller takes the answer
/// and releases the store before asking KMS anything.
#[must_use]
pub fn write_key(store: &ComAmazonawsGlueAwsglueState, catalog_id: &str) -> Option<String> {
    let at_rest = store
        .catalogs
        .get(catalog_id)?
        .encryption_settings
        .as_ref()?
        .encryption_at_rest
        .as_ref()?;
    if !KEYED_ENCRYPTION_MODES.contains(&at_rest.catalog_encryption_mode.as_str()) {
        return None;
    }
    at_rest
        .sse_aws_kms_key_id
        .clone()
        .filter(|key| !key.trim().is_empty())
}

/// Encrypt one catalog object: ask KMS for the data key the object is written under, under
/// the `key` the catalog's settings named.
///
/// `Ok(None)` is an object that depends on no key — the catalog encrypts its writes under
/// none, or KMS answered nothing about the one it names. `Err` carries KMS's OWN refusal
/// message, for the caller to answer `GlueEncryptionException` with: a catalog whose key
/// cannot be used cannot take the write either.
pub async fn seal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key: Option<&str>,
) -> Result<Option<ObjectEncryption>, String> {
    let Some(key) = key else {
        return Ok(None);
    };
    // `KeySpec` states the length of the data key: `AES_256`, a 256-bit AES key, the
    // length an envelope-encrypted object is keyed with. One of the two values the
    // modelled `DataKeySpec` enum publishes, and stated explicitly because the length is
    // not optional — "you must also specify the length of the data key. Use either the
    // KeySpec or NumberOfBytes parameters" (the operation's documentation).
    let error = match kms_client(ctx, region)
        .generate_data_key()
        .key_id(key)
        .key_spec(::aws_sdk_kms::types::DataKeySpec::Aes256)
        .send()
        .await
    {
        Ok(generated) => {
            let Some(blob) = generated.ciphertext_blob() else {
                ::tracing::warn!(
                    kms_key = key,
                    "KMS answered GenerateDataKey without an encrypted data key; the catalog \
                     object is recorded under no key"
                );
                return Ok(None);
            };
            return Ok(Some(ObjectEncryption {
                key_id: key.to_string(),
                encrypted_data_key: blob.as_ref().to_vec(),
            }));
        }
        Err(error) => error,
    };
    match judge(&error, USE_NEGATIVE_KMS_ERROR_CODES) {
        KmsAnswer::Refused(message) => Err(message),
        KmsAnswer::Silent(reason) => {
            ::tracing::warn!(
                kms_key = key,
                error = %reason,
                "KMS gave no answer about this catalog's key; the catalog object is recorded \
                 under no key"
            );
            Ok(None)
        }
    }
}

/// Decrypt one catalog object: recover the data key it was written under, which is what a
/// read has to do before it can answer the object.
///
/// `Ok(())` is a read that may be answered — the object was written under no key, or KMS
/// opened its data key, or KMS answered nothing about it. `Err` carries KMS's own refusal
/// message, for the caller to answer `GlueEncryptionException` with.
pub async fn unseal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    encryption: Option<&ObjectEncryption>,
) -> Result<(), String> {
    let Some(encryption) = encryption else {
        return Ok(());
    };
    let error = match kms_client(ctx, region)
        .decrypt()
        .ciphertext_blob(::aws_sdk_kms::primitives::Blob::new(
            encryption.encrypted_data_key.clone(),
        ))
        // The ciphertext itself names the key that sealed it; naming the key the object
        // records as well is what has KMS CHECK that assumption rather than open the
        // ciphertext under whatever key it was sealed with.
        .key_id(&encryption.key_id)
        .send()
        .await
    {
        Ok(_) => return Ok(()),
        Err(error) => error,
    };
    match judge(&error, USE_NEGATIVE_KMS_ERROR_CODES) {
        KmsAnswer::Refused(message) => Err(message),
        KmsAnswer::Silent(reason) => {
            ::tracing::warn!(
                kms_key = %encryption.key_id,
                error = %reason,
                "KMS gave no answer about the key this catalog object was written under; \
                 answering the read from what is stored"
            );
            Ok(())
        }
    }
}

/// The same for a read that answers MANY objects at once: every one of them has to be
/// decrypted before the answer is served, so the first refusal is the whole read's.
///
/// One KMS call per DISTINCT key: KMS's answer is about the key, so a page of objects
/// written under one key is one question, not one per object. That is also what bounds the
/// work a single read can ask of KMS — at most one question per object answered, and the
/// number of objects a read answers is itself bounded by the page size the model allows
/// (`CatalogGetterPageSize`, range 1..100, for the table listing; `BatchGetPartition`'s
/// own `PartitionsToGet`, length 0..1000) — and in practice one question per key the
/// catalog has been configured with.
pub async fn unseal_all(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    encryptions: &[ObjectEncryption],
) -> Result<(), String> {
    let mut asked: Vec<&str> = Vec::new();
    for encryption in encryptions {
        if asked.contains(&encryption.key_id.as_str()) {
            continue;
        }
        asked.push(&encryption.key_id);
        unseal(ctx, region, Some(encryption)).await?;
    }
    Ok(())
}

/// Resolve the `SseAwsKmsKeyId` a `PutDataCatalogEncryptionSettings` names, so a catalog
/// is not configured to encrypt its writes under a key that does not exist.
///
/// `Err` carries KMS's own refusal message for the caller's `InvalidInputException`. Only
/// a KMS answer about whether the key can be RESOLVED is a refusal
/// ([`EXISTENCE_NEGATIVE_KMS_ERROR_CODES`]): the state a key is in is read at each write
/// and read of an object, so a key that resolves may be configured whatever state it is
/// in.
pub async fn resolvable_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key: &str,
) -> Result<(), String> {
    let error = match kms_client(ctx, region)
        .describe_key()
        .key_id(key)
        .send()
        .await
    {
        Ok(_) => return Ok(()),
        Err(error) => error,
    };
    match judge(&error, EXISTENCE_NEGATIVE_KMS_ERROR_CODES) {
        KmsAnswer::Refused(message) => Err(message),
        KmsAnswer::Silent(reason) => {
            ::tracing::warn!(
                kms_key = key,
                error = %reason,
                "KMS gave no answer about the key this security configuration names; \
                 accepting it"
            );
            Ok(())
        }
    }
}

/// What KMS answered about a key, once the failure has been read.
enum KmsAnswer {
    /// KMS refused, and the refusal is about the KEY. Carries KMS's own message.
    Refused(String),
    /// KMS said nothing about the key. Carries why, for the operator-visible log line.
    Silent(String),
}

/// Read a KMS failure: a client-status answer carrying one of `negative` is a statement
/// about the key, anything else says nothing about it.
fn judge<E: ProvideErrorMetadata + ::std::error::Error + 'static>(
    error: &::aws_sdk_kms::error::SdkError<E, HttpResponse>,
    negative: &[&str],
) -> KmsAnswer {
    let reason = ::aws_smithy_types::error::display::DisplayErrorContext(error).to_string();
    // The status KMS answered with, when it answered at all: a 5xx is about KMS, not
    // about the key, whatever code travels with it.
    let client_error = error
        .raw_response()
        .is_some_and(|response| response.status().is_client_error());
    let code = ProvideErrorMetadata::code(error).unwrap_or_default();
    if client_error && negative.iter().any(|known| known.eq_ignore_ascii_case(code)) {
        // KMS's own wording for the refusal, which is what the catalog relays; a coded
        // answer that carries no message at all leaves the rendered failure as the next
        // best account of it.
        let message = ProvideErrorMetadata::message(error)
            .map_or_else(|| reason.clone(), str::to_string);
        return KmsAnswer::Refused(message);
    }
    KmsAnswer::Silent(reason)
}

/// The KMS error codes that say the key cannot be USED for a cryptographic operation
/// right now: it does not exist or is not an identifier KMS can resolve
/// (`NotFoundException`, `InvalidArnException`), this caller may not use it
/// (`AccessDeniedException`), it is disabled (`DisabledException`), it is in another state
/// that forbids use such as pending deletion or pending import
/// (`KMSInvalidStateException`), its backing material is unavailable
/// (`KeyUnavailableException`), it cannot perform the operation
/// (`InvalidKeyUsageException`), or it is not the key that sealed this object
/// (`IncorrectKeyException`, `InvalidCiphertextException`).
///
/// A code outside this list, or no code at all, is not a statement about the key: a
/// throttle, a server fault, or a reply that never came from KMS (as when no KMS is
/// mounted beside this service).
const USE_NEGATIVE_KMS_ERROR_CODES: &[&str] = &[
    "AccessDeniedException",
    "DisabledException",
    "IncorrectKeyException",
    "InvalidArnException",
    "InvalidCiphertextException",
    "InvalidKeyUsageException",
    "KMSInvalidStateException",
    "KeyUnavailableException",
    "NotFoundException",
];

/// The subset of [`USE_NEGATIVE_KMS_ERROR_CODES`] that says the identifier RESOLVES to no
/// key this caller can read at all — the two codes a key lookup answers for an identifier
/// it cannot resolve, plus the authorization refusal for a key this caller may not read.
///
/// Deliberately NOT the state codes: a key that resolves exists, and what state it is in
/// is read at each write and each read of an object encrypted under it.
const EXISTENCE_NEGATIVE_KMS_ERROR_CODES: &[&str] = &[
    "AccessDeniedException",
    "InvalidArnException",
    "NotFoundException",
];

/// A KMS client over the in-process callback into this same emulator.
///
/// The callback is the framework's documented seam for calling another emulated service:
/// it routes in process, so no network client is configured here. `region` is the
/// request's own signing region, so the key is looked up where the catalog that names it
/// lives.
///
/// The transport is in-process, so there is nothing to retry, time out or watch for a
/// stalled stream — and each of those would need a sleep implementation this client is not
/// given. The same goes for caching the identity: the credentials the callback hands over
/// are used directly.
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
        .stalled_stream_protection(
            ::aws_sdk_kms::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_kms::Client::from_conf(config)
}
