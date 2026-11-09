// SPDX-License-Identifier: CC0-1.0

//! The API key credential providers of this account's token vault: the record a
//! create persists, the ARNs it mints, and the Secrets Manager secret the API key
//! itself lives in.
//!
//! One concern, kept beside the token vault rather than inside one `ops/` file:
//! the provider's own ARN, the secret its key is stored under and the response
//! envelopes are shared by the create and by every later read, so two operations
//! cannot report one provider two ways.
//!
//! ## Where the API key lives
//!
//! The key is NOT held in this service's own store. A provider carries a
//! `MANAGED` or an `EXTERNAL` secret source:
//!
//! * `MANAGED` — "the secret is managed by the service" — so the create itself
//!   puts the caller's key into a secret in Secrets Manager, under a name derived
//!   from the vault and the provider ([`managed_api_key_secret_name`]), as the
//!   JSON document a secret's value is ([`managed_api_key_secret_value`]), and
//!   reports that secret's ARN together with the key that document carries the
//!   API key under. The ARN is part of the create's own answer, so it names a
//!   secret Secrets Manager really holds: a client that reads the provider and
//!   then reads the secret finds the key there;
//! * `EXTERNAL` — "you manage the secret yourself in Amazon Web Services Secrets
//!   Manager" — so the create READS the secret the request's `apiKeySecretConfig`
//!   names and reports the ARN Secrets Manager answered with, together with the
//!   `jsonKey` the caller extracts the key with. Nothing is written into it: the
//!   key is the caller's. The read is `GetSecretValue`, which DECRYPTS: a secret
//!   whose KMS key will not decrypt is the modelled `DecryptionFailure`, because
//!   a key the provider cannot read is of no use to it.
//!
//! Secrets Manager is reached the way this crate reaches every other emulated
//! service — through the in-process callback
//! ([`smithy_server_runtime::OperationContext::callback`]) — so this crate names
//! no other service crate.

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    CreateApiKeyCredentialProviderOutput, GetApiKeyCredentialProviderOutput,
    ResourceNotFoundException, Secret,
};

use super::{timestamp, DEFAULT_REGION};

/// The ARN namespace a credential provider of the token vault carries, which is
/// NOT this service's own: `ApiKeyCredentialProviderArnType` is
/// `^arn:(aws|aws-us-gov):acps:…:token-vault/…/apikeycredentialprovider/…$`
/// (`smithy_query shape=ApiKeyCredentialProviderArnType`).
pub const CREDENTIAL_PROVIDER_ARN_SERVICE: &str = "acps";

/// The ARN path segment an API key credential provider of a token vault is
/// addressed under, from that same pattern.
pub const API_KEY_CREDENTIAL_PROVIDER_ARN_TYPE: &str = "apikeycredentialprovider";

/// The resource noun [`super::resource_not_found`] names for a SECRET this
/// service could not resolve for a provider.
pub const RESOURCE_SECRET: &str = "secret";

/// How this service words the 404 for an API key credential provider this
/// account does not hold: the resource type and the name the request addressed it
/// by ("ApiKeyCredentialProvider not found for <name>").
///
/// The credential-provider routes do NOT take the shared
/// [`super::resource_not_found`] sentence — they name the resource type and echo
/// the `name` the caller asked for, which is the identifier those routes address
/// a provider by. Shared by every operation that resolves a provider, so one
/// missing provider cannot be reported in two sentences.
pub fn api_key_credential_provider_not_found(name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("ApiKeyCredentialProvider not found for {name}")),
    }
}

/// `SecretSourceType` values. The enum's full value list is `MANAGED | EXTERNAL`
/// (`smithy_query shape=SecretSourceType`).
pub const SECRET_SOURCE_MANAGED: &str = "MANAGED";
pub const SECRET_SOURCE_EXTERNAL: &str = "EXTERNAL";

/// The request members whose rules the model states in prose, so the framework's
/// constraint walk cannot refuse them and a handler must.
pub const API_KEY_MEMBER: &str = "apiKey";
pub const API_KEY_SECRET_CONFIG_MEMBER: &str = "apiKeySecretConfig";

/// How many API key credential providers this emulator will hold.
///
/// `CreateApiKeyCredentialProvider` models `ServiceQuotaExceededException` (HTTP
/// 402) for exceeding the real service's quota; this number is not a fidelity
/// claim about that quota but the emulator's own ceiling, set generously so no
/// realistic workflow meets it, so a caller looping on create cannot grow this
/// process's memory without bound.
pub const MAX_API_KEY_CREDENTIAL_PROVIDERS: usize = 1_000;

/// The prefix the service holds its OWN managed secrets under in the caller's
/// Secrets Manager, so a managed secret is recognisable as this service's and
/// cannot collide with a name a client chose for a secret of its own.
pub const MANAGED_SECRET_PREFIX: &str = "bedrock-agentcore-identity";

/// The segment of a managed secret's name that says what is in it.
const MANAGED_SECRET_KIND: &str = "apikey";

/// The JSON key the service's own managed secret holds the API key under, and
/// therefore the `apiKeySecretJsonKey` a managed provider reports: a secret's
/// value is a JSON document, so the key is extracted from it by name exactly as
/// it is from a secret the caller shaped.
pub const MANAGED_SECRET_JSON_KEY: &str = "apiKey";

/// The value the secret holding a `MANAGED` provider's key is stored with: the
/// JSON document carrying the caller's key under [`MANAGED_SECRET_JSON_KEY`].
pub fn managed_api_key_secret_value(api_key: &str) -> String {
    ::serde_json::json!({ MANAGED_SECRET_JSON_KEY: api_key }).to_string()
}

/// The name the secret holding a `MANAGED` provider's API key is created under:
/// the service's own prefix, the token vault the provider belongs to, and the
/// provider's name, so the name is a function of the resource rather than of
/// anything else the caller sent.
///
/// Every character of it is one Secrets Manager admits in a secret name (ASCII
/// letters, numbers and `/_+=.@-`, which `CreateSecret` refuses anything outside
/// of), and a `CredentialProviderName` is `^[a-zA-Z0-9\-_]+$` of at most 128
/// characters (`smithy_query shape=CredentialProviderName`), so the whole name
/// stays well inside that operation's 1..512 length.
pub fn managed_api_key_secret_name(token_vault_id: &str, provider_name: &str) -> String {
    format!("{MANAGED_SECRET_PREFIX}/{token_vault_id}/{MANAGED_SECRET_KIND}/{provider_name}")
}

/// `arn:aws:acps:{region}:{account}:token-vault/{tokenVaultId}/apikeycredentialprovider/{name}`
/// — `ApiKeyCredentialProviderArnType`'s pattern, which nests the provider under
/// the token vault that holds it and ends in the name the caller chose.
pub fn api_key_credential_provider_arn(
    region: &str,
    token_vault_id: &str,
    name: &str,
) -> String {
    ::engine::emulator_arn(
        CREDENTIAL_PROVIDER_ARN_SERVICE,
        region,
        &format!("token-vault/{token_vault_id}/{API_KEY_CREDENTIAL_PROVIDER_ARN_TYPE}/{name}"),
    )
}

/// One API key credential provider of this account, as
/// `CreateApiKeyCredentialProvider` persisted it.
///
/// The name is the identifier: the read, the update and the delete all address a
/// provider by `name`, and it "must be unique within your account"
/// (`smithy_query doc shape=CreateApiKeyCredentialProviderRequest member=name`),
/// so the store is keyed by it.
#[derive(Debug, Clone)]
pub struct ApiKeyCredentialProviderRecord {
    pub name: String,
    pub credential_provider_arn: String,
    /// The ARN of the secret the key lives in: the one this create minted for a
    /// `MANAGED` provider, or the one Secrets Manager reported for the secret an
    /// `EXTERNAL` provider references.
    pub api_key_secret_arn: String,
    /// The JSON key the API key is extracted from that secret with: the one the
    /// caller named for a secret they shaped, and the service's own
    /// ([`MANAGED_SECRET_JSON_KEY`]) for the secret it manages.
    pub api_key_secret_json_key: Option<String>,
    /// The effective [`SECRET_SOURCE_MANAGED`] / [`SECRET_SOURCE_EXTERNAL`].
    pub api_key_secret_source: String,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ApiKeyCredentialProviderRecord {
    /// This provider as `CreateApiKeyCredentialProvider`'s response envelope.
    pub fn create_output(&self) -> CreateApiKeyCredentialProviderOutput {
        CreateApiKeyCredentialProviderOutput {
            api_key_secret_arn: Secret {
                secret_arn: self.api_key_secret_arn.clone(),
            },
            api_key_secret_json_key: self.api_key_secret_json_key.clone(),
            api_key_secret_source: Some(self.api_key_secret_source.clone()),
            name: self.name.clone(),
            credential_provider_arn: self.credential_provider_arn.clone(),
        }
    }

    /// This provider as `GetApiKeyCredentialProvider`'s response envelope — the
    /// members [`Self::create_output`] renders plus the two instants the read
    /// reports, off the one record, so a create and a later read cannot disagree.
    pub fn get_output(&self) -> GetApiKeyCredentialProviderOutput {
        let created = self.create_output();
        GetApiKeyCredentialProviderOutput {
            api_key_secret_arn: created.api_key_secret_arn,
            api_key_secret_json_key: created.api_key_secret_json_key,
            api_key_secret_source: created.api_key_secret_source,
            name: created.name,
            credential_provider_arn: created.credential_provider_arn,
            created_time: timestamp(self.created_at),
            last_updated_time: timestamp(self.updated_at),
        }
    }
}

/// The secret source a request asks for: the one it named, else the one its
/// members imply.
///
/// A request that references a secret of its own
/// (`apiKeySecretConfig`, "Required when apiKeySecretSource is set to EXTERNAL")
/// is asking for that secret to be used; anything else is the service managing
/// the secret it stores the caller's `apiKey` in.
pub fn effective_secret_source(requested: Option<&str>, has_secret_config: bool) -> &str {
    match requested {
        Some(SECRET_SOURCE_EXTERNAL) => SECRET_SOURCE_EXTERNAL,
        Some(SECRET_SOURCE_MANAGED) => SECRET_SOURCE_MANAGED,
        _ if has_secret_config => SECRET_SOURCE_EXTERNAL,
        _ => SECRET_SOURCE_MANAGED,
    }
}

/// The message this service refuses a credential provider with when a secret
/// the caller references cannot be decrypted.
///
/// Fixed text: nothing of the request or of the state is interpolated, because
/// the remedy does not depend on which secret or which key was at fault — the
/// caller fixes the key's permissions and retries. Shared by every credential
/// provider plane of this crate, so one undecryptable secret is reported in one
/// sentence whichever provider referenced it.
pub const SECRET_UNDECRYPTABLE_MESSAGE: &str =
    "Unable to decrypt the provided secret. Verify KMS key permissions.";

/// Why the secret an API key credential provider's key lives in could not be
/// reported.
#[derive(Debug)]
pub enum ApiKeySecretProblem {
    /// Secrets Manager answered that this account holds no such secret, so the
    /// request references a resource that does not exist.
    Absent,
    /// Secrets Manager will not decrypt the secret: the state of its own KMS
    /// key. The secret is there, but its value cannot be read, so a credential
    /// provider over it would carry a credential nothing can use.
    Undecryptable,
    /// Secrets Manager could not be asked, or refused the call: nothing is known
    /// about the secret, so there is no ARN to report. Carries the rendering of
    /// what came back, for the trace.
    Unavailable(String),
}

/// Put `api_key` into the Secrets Manager secret a `MANAGED` provider's key lives
/// in, and report that secret's ARN.
///
/// The secret is created under [`managed_api_key_secret_name`], carrying the key
/// as the JSON document [`managed_api_key_secret_value`] builds; the store itself
/// is the shared one every credential-provider plane of this crate uses
/// ([`store_managed_secret`]), so the ARN reported is the one Secrets Manager
/// answered with.
///
/// Call it with NO state guard held — it awaits.
pub async fn store_managed_api_key(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    token_vault_id: &str,
    provider_name: &str,
    api_key: &str,
) -> Result<String, ApiKeySecretProblem> {
    store_managed_secret(
        ctx,
        region,
        &managed_api_key_secret_name(token_vault_id, provider_name),
        &managed_api_key_secret_value(api_key),
    )
    .await
}

/// Put `value` into the Secrets Manager secret named `name`, which is a secret
/// this SERVICE manages for a credential provider, and report that secret's ARN.
///
/// A secret already standing under that name is the provider's own — the name is
/// derived from the provider — so the value is put into it as a new version
/// rather than refused. Shared by the credential-provider stores of this crate
/// (the API key provider above and the payment provider beside it), so no two of
/// them differ about how a managed secret is written or which ARN is reported
/// for it.
///
/// Call it with NO state guard held — it awaits.
pub(crate) async fn store_managed_secret(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    name: &str,
    value: &str,
) -> Result<String, ApiKeySecretProblem> {
    let client = secrets_manager_client(ctx, region);
    let created = client
        .create_secret()
        .name(name)
        .secret_string(value)
        .send()
        .await;
    match created {
        Ok(answer) => arn_of(answer.arn, "CreateSecret"),
        Err(error)
            if error
                .as_service_error()
                .is_some_and(|service| service.is_resource_exists_exception()) =>
        {
            let put = client
                .put_secret_value()
                .secret_id(name)
                .secret_string(value)
                .send()
                .await
                .map_err(|error| {
                    unavailable(::aws_smithy_types::error::display::DisplayErrorContext(&error))
                })?;
            arn_of(put.arn, "PutSecretValue")
        }
        Err(error) => Err(unavailable(
            ::aws_smithy_types::error::display::DisplayErrorContext(&error),
        )),
    }
}

/// The ARN of the secret an `EXTERNAL` credential provider references, as
/// Secrets Manager reports it for the `secretId` the caller named — which may be
/// the secret's name or its whole ARN, both of which that service resolves.
///
/// Nothing is written: the secret is the caller's. The read is `GetSecretValue`,
/// the call that DECRYPTS the value, because a credential the provider cannot
/// read is of no use to it: a secret whose KMS key will not decrypt is
/// [`ApiKeySecretProblem::Undecryptable`] and a `secretId` Secrets Manager holds
/// no secret for is [`ApiKeySecretProblem::Absent`]. The ARN reported is the one
/// Secrets Manager answered with, never the string the request carried, so a
/// provider is never recorded against a secret nothing holds.
///
/// Shared by every credential-provider plane of this crate, so none of them can
/// accept a secret another would refuse.
///
/// Call it with NO state guard held — it awaits.
pub async fn referenced_external_secret_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    secret_id: &str,
) -> Result<String, ApiKeySecretProblem> {
    match secrets_manager_client(ctx, region)
        .get_secret_value()
        .secret_id(secret_id)
        .send()
        .await
    {
        Ok(answer) => arn_of(answer.arn, "GetSecretValue"),
        Err(error) => {
            let service = error.as_service_error();
            if service.is_some_and(|failure| failure.is_decryption_failure()) {
                return Err(ApiKeySecretProblem::Undecryptable);
            }
            if service.is_some_and(|failure| failure.is_resource_not_found_exception()) {
                return Err(ApiKeySecretProblem::Absent);
            }
            Err(unavailable(
                ::aws_smithy_types::error::display::DisplayErrorContext(&error),
            ))
        }
    }
}

/// The ARN a Secrets Manager answer reported, or [`ApiKeySecretProblem`] when it
/// reported none: the provider's own answer names that secret, so there is
/// nothing to report without it.
fn arn_of(arn: Option<String>, operation: &str) -> Result<String, ApiKeySecretProblem> {
    arn.ok_or_else(|| {
        ApiKeySecretProblem::Unavailable(format!("{operation} answered with no secret ARN"))
    })
}

/// A failed Secrets Manager call as [`ApiKeySecretProblem::Unavailable`], with
/// what came back rendered for the trace.
pub(crate) fn unavailable(detail: impl ::std::fmt::Display) -> ApiKeySecretProblem {
    ApiKeySecretProblem::Unavailable(detail.to_string())
}

/// The Secrets Manager client this crate's credential-provider stores store and
/// resolve a secret through: the in-process callback into this same emulator, at
/// the endpoint of the REQUEST's own Region, so the secret lands where the
/// provider's own ARN says the provider is.
pub(crate) fn secrets_manager_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_secretsmanager::Client {
    let region = if region.is_empty() {
        DEFAULT_REGION.to_string()
    } else {
        region.to_string()
    };
    let config = ::aws_sdk_secretsmanager::Config::builder()
        .behavior_version(::aws_sdk_secretsmanager::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_secretsmanager::config::Region::new(
            region.clone(),
        ))
        .endpoint_url(format!("https://secretsmanager.{region}.amazonaws.com"))
        // The transport is in-process: there is nothing to retry, time out or watch
        // for a stalled stream, and each of those would need a sleep implementation
        // this client is not given. The same goes for caching the identity: the
        // credentials are handed over directly.
        .retry_config(::aws_sdk_secretsmanager::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_secretsmanager::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_secretsmanager::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_secretsmanager::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_secretsmanager::Client::from_conf(config)
}
