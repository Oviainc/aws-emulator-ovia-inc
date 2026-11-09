// SPDX-License-Identifier: CC0-1.0

//! The PAYMENT credential providers of this account's token vault: the record a
//! create persists, the ARN it mints, and the Secrets Manager secrets the
//! credentials themselves live in.
//!
//! One concern, kept beside the token vault rather than inside one `ops/` file:
//! the provider's own ARN, the secrets its credentials are stored under and the
//! response envelopes are shared by the create and by every later read, so two
//! operations cannot report one provider two ways.
//!
//! ## Where the credentials live
//!
//! Not in this service's own store. Each vendor needs two secrets, and each of
//! them carries a `MANAGED` or an `EXTERNAL` source, exactly as the API key
//! provider beside it does ([`super::api_key_credential_providers`], whose
//! Secrets Manager helpers this module shares):
//!
//! * `MANAGED` — "the secret is managed by the service" — so the create itself
//!   puts the value the caller handed over into a secret in Secrets Manager,
//!   under a name derived from the vault, the provider and the member
//!   ([`managed_payment_secret_name`]), as the JSON document a secret's value is,
//!   and reports that secret's ARN together with the key that document carries
//!   the value under. A client that reads the provider and then reads the secret
//!   finds the credential there;
//! * `EXTERNAL` — "you manage the secret yourself in Amazon Web Services Secrets
//!   Manager" — so the create READS the secret the request references and
//!   reports the ARN Secrets Manager answered with, together with the `jsonKey`
//!   the caller extracts the value with. Nothing is written into it: the secret
//!   is the caller's. The read is `GetSecretValue`, which DECRYPTS: a secret
//!   whose KMS key will not decrypt is the modelled `DecryptionFailure`, which
//!   is what makes a provider's credentials usable or not.
//!
//! So no ARN here is composed from a request: a provider reports the ARN its
//! secret's owner answered with, and `PaymentProviderConfigurationOutput` is
//! "no raw secrets, only ARNs".

use std::collections::HashMap;

use super::api_key_credential_providers::{
    referenced_external_secret_arn, store_managed_secret, ApiKeySecretProblem,
    CREDENTIAL_PROVIDER_ARN_SERVICE, MANAGED_SECRET_PREFIX, SECRET_SOURCE_EXTERNAL,
    SECRET_SOURCE_MANAGED,
};
use super::*;
use crate::ops::types::{
    CoinbaseCdpConfigurationOutput, CreatePaymentCredentialProviderOutput,
    GetPaymentCredentialProviderOutput, PaymentProviderConfigurationInput,
    PaymentProviderConfigurationOutput, ResourceNotFoundException, Secret, SecretReference,
    StripePrivyConfigurationOutput, ValidationException,
};

/// The resource noun [`resource_not_found`] names for a payment credential
/// provider.
pub const RESOURCE_PAYMENT_CREDENTIAL_PROVIDER: &str = "payment credential provider";

/// How this service words a payment credential provider no name of this account
/// resolves to: the resource's own modelled name, then that it is not there.
///
/// Entirely fixed text — the name the request carried is the caller's own value
/// and the remedy does not depend on it.
pub const PAYMENT_CREDENTIAL_PROVIDER_NOT_FOUND_MESSAGE: &str =
    "PaymentCredentialProvider not found";

/// The modelled not-found error for a payment credential provider this account
/// does not hold.
///
/// Shared by the handlers rather than written per operation, so one missing
/// provider cannot be reported in two different sentences by two operations.
pub fn payment_credential_provider_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(PAYMENT_CREDENTIAL_PROVIDER_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The ARN resource type a provider's name is nested under, inside the token
/// vault that holds its credentials (`PaymentCredentialProviderArnType`'s
/// pattern,
/// `^arn:(aws|aws-us-gov):acps:[A-Za-z0-9-]{1,64}:[0-9]{12}:token-vault/[a-zA-Z0-9-.]+/paymentcredentialprovider/[a-zA-Z0-9-.]+$`).
pub const PAYMENT_CREDENTIAL_PROVIDER_ARN_TYPE: &str = "paymentcredentialprovider";

/// `PaymentCredentialProviderVendorType`'s full value list
/// (`crate::ops::types::PAYMENT_CREDENTIAL_PROVIDER_VENDOR_TYPE_VALUES`, from
/// `smithy_query shape=PaymentCredentialProviderVendorType`): the vendor decides
/// which member of `PaymentProviderConfigurationInput` the request carries.
pub const PAYMENT_VENDOR_COINBASE_CDP: &str = "CoinbaseCDP";
pub const PAYMENT_VENDOR_STRIPE_PRIVY: &str = "StripePrivy";

/// The `PaymentProviderConfigurationInput` / `…Output` member each vendor's
/// configuration is carried in (`smithy_query
/// shape=PaymentProviderConfigurationInput`).
const COINBASE_CDP_CONFIGURATION_MEMBER: &str = "coinbaseCdpConfiguration";
const STRIPE_PRIVY_CONFIGURATION_MEMBER: &str = "stripePrivyConfiguration";

/// The request member that carries the vendor's configuration, named as the wire
/// spells it, so a refusal of it names what the caller sent.
const PROVIDER_CONFIGURATION_INPUT_MEMBER: &str = "providerConfigurationInput";

/// The secret members of `CoinbaseCdpConfigurationInput` and
/// `StripePrivyConfigurationInput`, named as the wire spells them: the value,
/// the source that says who manages it and the reference to the caller's own
/// secret (`smithy_query shape=CoinbaseCdpConfigurationInput` /
/// `shape=StripePrivyConfigurationInput`).
const API_KEY_SECRET_MEMBERS: SecretMembers = SecretMembers {
    value: "apiKeySecret",
    source: "apiKeySecretSource",
    config: "apiKeySecretConfig",
};
const WALLET_SECRET_MEMBERS: SecretMembers = SecretMembers {
    value: "walletSecret",
    source: "walletSecretSource",
    config: "walletSecretConfig",
};
const APP_SECRET_MEMBERS: SecretMembers = SecretMembers {
    value: "appSecret",
    source: "appSecretSource",
    config: "appSecretConfig",
};
const AUTHORIZATION_PRIVATE_KEY_MEMBERS: SecretMembers = SecretMembers {
    value: "authorizationPrivateKey",
    source: "authorizationPrivateKeySource",
    config: "authorizationPrivateKeyConfig",
};

/// How many payment credential providers this emulator will hold.
///
/// `CreatePaymentCredentialProvider` models both
/// `ResourceLimitExceededException` and `ServiceQuotaExceededException` for
/// exceeding the real service's limits; this number is not a fidelity claim
/// about either, but the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_PAYMENT_CREDENTIAL_PROVIDERS: usize = 1_000;

/// `arn:aws:acps:{region}:{account}:token-vault/{tokenVaultId}/paymentcredentialprovider/{name}`
/// — `PaymentCredentialProviderArnType`'s pattern, which nests the provider
/// under the token vault holding its credentials and ends in the name the caller
/// chose.
pub fn payment_credential_provider_arn(region: &str, token_vault_id: &str, name: &str) -> String {
    ::engine::emulator_arn(
        CREDENTIAL_PROVIDER_ARN_SERVICE,
        region,
        &format!("token-vault/{token_vault_id}/{PAYMENT_CREDENTIAL_PROVIDER_ARN_TYPE}/{name}"),
    )
}

/// The name the secret holding ONE `MANAGED` secret of a payment provider is
/// created under: the service's own prefix, the token vault, the provider and the
/// member the secret holds — so the name is a function of the resource, and the
/// two secrets one vendor needs never collide.
///
/// Every character of it is one Secrets Manager admits in a secret name (ASCII
/// letters, numbers and `/_+=.@-`), as the API key provider's own managed name
/// is, and a `CredentialProviderName` is at most 128 characters of
/// `^[a-zA-Z0-9\-_]+$`, so the whole name stays well inside `CreateSecret`'s
/// 1..512 length.
pub fn managed_payment_secret_name(
    token_vault_id: &str,
    provider_name: &str,
    member: &str,
) -> String {
    format!(
        "{MANAGED_SECRET_PREFIX}/{token_vault_id}/{PAYMENT_CREDENTIAL_PROVIDER_ARN_TYPE}/\
         {provider_name}/{member}"
    )
}

/// The value a managed payment secret is stored with: the JSON document carrying
/// the caller's value under the member it was sent as, which is therefore the
/// JSON key the provider reports for it.
pub fn managed_payment_secret_value(member: &str, value: &str) -> String {
    ::serde_json::json!({ member: value }).to_string()
}

/// Whether `character` is admitted in a secret's name.
///
/// `SecretArn`'s pattern ends in `[a-zA-Z0-9-_/+=.@!]+`, which is the set
/// Secrets Manager accepts a secret name from, so a `secretId` carrying anything
/// else names no secret and is refused on the request rather than asked about.
fn is_secret_name_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || "-_/+=.@!".contains(character)
}

/// The `SecretId` a reference may be ASKED about, or `None` when the value names
/// no secret this account's token vault can hold.
///
/// A provider's credentials live in the token vault of THIS account, in the
/// Region the request was signed for, so the secrets it can reference are that
/// account's and that Region's: a reference is either a secret's name, or the
/// whole ARN of a secret of this account in this Region
/// (`::service_common::arn::SECRETS_MANAGER_SECRET`, whose account and region
/// fields AWS leaves optional on a well-formed ARN). Anything else — another
/// account's secret, another Region's, another service's resource, or a name
/// outside the characters a secret's name is made of — is refused on the request
/// instead of being asked about, so what this service resolves is bounded by the
/// account and Region it serves rather than by the request.
fn addressable_secret_id(secret_id: &str, region: &str) -> Option<String> {
    if secret_id.is_empty() {
        return None;
    }
    if secret_id.starts_with("arn:") || ::service_common::arn::has_arn_fields(secret_id) {
        let arn = ::service_common::arn::parse(secret_id)?;
        if !::service_common::arn::SECRETS_MANAGER_SECRET.admits(secret_id)
            || (!arn.account.is_empty() && arn.account != ::engine::emulator_account_id())
            || (!arn.region.is_empty() && arn.region != region)
        {
            return None;
        }
        return Some(secret_id.to_string());
    }
    secret_id
        .chars()
        .all(is_secret_name_character)
        .then(|| secret_id.to_string())
}

/// The three members that describe ONE secret of a vendor configuration, named
/// as the wire spells them.
#[derive(Debug, Clone, Copy)]
struct SecretMembers {
    value: &'static str,
    source: &'static str,
    config: &'static str,
}

/// What a request asks the vault to do about ONE secret of a vendor
/// configuration, once the rules on its members are satisfied.
#[derive(Debug, Clone)]
enum RequestedSecretTarget {
    /// The SERVICE manages the secret: the value to put into it.
    Managed(String),
    /// The CALLER manages the secret: the `secretId` the request referenced it
    /// by — a name or a whole ARN, both of which Secrets Manager resolves — and
    /// the JSON key the value is extracted under.
    External { secret_id: String, json_key: String },
}

/// ONE secret of a vendor configuration as the REQUEST asks for it: which
/// members described it, and what is to become of it.
#[derive(Debug, Clone)]
pub struct RequestedPaymentSecret {
    members: SecretMembers,
    target: RequestedSecretTarget,
}

impl RequestedPaymentSecret {
    /// What the request asks for one secret, or the modelled validation failure
    /// its members are refused for.
    ///
    /// A secret the CALLER manages needs the reference that names it: its
    /// `*Config` is "required when `*Source` is set to `EXTERNAL`"
    /// (`smithy_query shape=CoinbaseCdpConfigurationInput`), and that reference
    /// has to name a secret of THIS account in the Region the request was signed
    /// for ([`addressable_secret_id`]) — the provider's credentials live in this
    /// account's regional token vault, so a secret of another account or Region
    /// is not a secret it can hold. A secret the
    /// SERVICE manages is the place the value is stored, so the request must
    /// carry that value — the value members have a modelled default of `""`, so
    /// a request that leaves one out is indistinguishable from one that sends it
    /// empty, and either way there is nothing to store.
    fn from_members(
        members: SecretMembers,
        region: &str,
        value: Option<&String>,
        source: Option<&String>,
        config: Option<&SecretReference>,
    ) -> Result<Self, ValidationException> {
        let source = effective_secret_source(source.map(String::as_str), config.is_some());
        if source == SECRET_SOURCE_EXTERNAL {
            let reference = config.ok_or_else(|| {
                field_validation_failed(
                    members.config,
                    &format!(
                        "must be provided when {} is {SECRET_SOURCE_EXTERNAL}",
                        members.source
                    ),
                )
            })?;
            let secret_id = addressable_secret_id(&reference.secret_id, region).ok_or_else(|| {
                field_validation_failed(
                    &format!("{}.secretId", members.config),
                    &format!(
                        "must be the name of a secret of this account in {region}, or {} of this \
                         account in {region}",
                        ::service_common::arn::SECRETS_MANAGER_SECRET.phrase()
                    ),
                )
            })?;
            return Ok(Self {
                members,
                target: RequestedSecretTarget::External {
                    secret_id,
                    json_key: reference.json_key.clone(),
                },
            });
        }
        let value = value.map_or("", String::as_str);
        if value.is_empty() {
            return Err(field_validation_failed(
                members.value,
                &format!(
                    "must be provided when {} is {SECRET_SOURCE_MANAGED}",
                    members.source
                ),
            ));
        }
        Ok(Self {
            members,
            target: RequestedSecretTarget::Managed(value.to_string()),
        })
    }

    /// Carry out what the request asked for, against the service that owns the
    /// secrets, and report what the provider is to carry for this one.
    ///
    /// Call it with NO state guard held — it awaits.
    async fn resolve(
        self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
        token_vault_id: &str,
        provider_name: &str,
    ) -> Result<StoredPaymentSecret, PaymentSecretProblem> {
        match self.target {
            RequestedSecretTarget::Managed(value) => {
                let secret_arn = store_managed_secret(
                    ctx,
                    region,
                    &managed_payment_secret_name(token_vault_id, provider_name, self.members.value),
                    &managed_payment_secret_value(self.members.value, &value),
                )
                .await?;
                Ok(StoredPaymentSecret {
                    secret_arn,
                    // The managed secret carries the value under the member it
                    // was sent as, which is what a client extracts it with.
                    json_key: Some(self.members.value.to_string()),
                    source: SECRET_SOURCE_MANAGED.to_string(),
                })
            }
            RequestedSecretTarget::External {
                secret_id,
                json_key,
            } => Ok(StoredPaymentSecret {
                secret_arn: referenced_secret_arn(ctx, region, &secret_id).await?,
                json_key: Some(json_key),
                source: SECRET_SOURCE_EXTERNAL.to_string(),
            }),
        }
    }
}

/// Why the secrets a payment credential provider authenticates with could not be
/// reported.
#[derive(Debug)]
pub enum PaymentSecretProblem {
    /// Secrets Manager answered that this account holds no secret the request
    /// referenced, so the request references a resource that does not exist.
    Absent,
    /// Secrets Manager will not decrypt the secret: the state of its own KMS key.
    Undecryptable,
    /// Secrets Manager could not be asked, or refused the call for a reason that
    /// says nothing about the secret: nothing is known about it, so there is no
    /// ARN to report. Carries the rendering of what came back, for the trace.
    Unavailable(String),
}

impl From<ApiKeySecretProblem> for PaymentSecretProblem {
    /// The outcome of a Secrets Manager call made through the shared helpers,
    /// reported in this plane's own terms.
    fn from(problem: ApiKeySecretProblem) -> Self {
        match problem {
            ApiKeySecretProblem::Absent => Self::Absent,
            ApiKeySecretProblem::Undecryptable => Self::Undecryptable,
            ApiKeySecretProblem::Unavailable(detail) => Self::Unavailable(detail),
        }
    }
}

/// The ARN of the secret a caller's own `SecretReference` names, as AWS Secrets
/// Manager reports it for the `secretId` the request carried — which may be the
/// secret's name or its whole ARN, both of which that service resolves.
///
/// WHAT IS ASKED, AND WHERE. The `secretId` reaching here has already been
/// confined to a secret of THIS account in the request's own Region
/// ([`addressable_secret_id`]), and the resolution itself is the one every
/// credential-provider plane of this crate shares
/// ([`referenced_external_secret_arn`]), whose client is the in-process callback
/// into this same emulator: the request is routed in process to the emulated
/// Secrets Manager mounted beside this service, so no socket is opened, no host
/// is dialled and nothing outside this emulator is reachable from here.
///
/// Nothing is written: the secret is the caller's. The shared read DECRYPTS,
/// because a credential a payment connector cannot read is of no use to it, so a
/// key Secrets Manager will not decrypt under is
/// [`PaymentSecretProblem::Undecryptable`] and a `secretId` it holds no secret
/// for is [`PaymentSecretProblem::Absent`].
///
/// Call it with NO state guard held — it awaits.
async fn referenced_secret_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    secret_id: &str,
) -> Result<String, PaymentSecretProblem> {
    referenced_external_secret_arn(ctx, region, secret_id)
        .await
        .map_err(PaymentSecretProblem::from)
}

/// ONE secret of a stored vendor configuration, as every response that reports
/// the provider carries it: who manages it, the ARN its owner holds it at and the
/// JSON key the value is extracted under. The VALUE is not here — it is in
/// Secrets Manager.
#[derive(Debug, Clone)]
pub struct StoredPaymentSecret {
    /// The ARN Secrets Manager answered with for this secret.
    pub secret_arn: String,
    /// The JSON key the value is extracted from that secret with: the one the
    /// caller named for a secret they shaped, and the member's own name for the
    /// secret this service manages.
    pub json_key: Option<String>,
    /// The EFFECTIVE [`SECRET_SOURCE_MANAGED`] / [`SECRET_SOURCE_EXTERNAL`].
    pub source: String,
}

impl StoredPaymentSecret {
    /// The secret as a response reports it: its ARN and nothing else.
    fn reported(&self) -> Secret {
        Secret {
            secret_arn: self.secret_arn.clone(),
        }
    }
}

/// The modelled validation error for a request whose `providerConfigurationInput`
/// is not the member its `credentialProviderVendor` requires.
///
/// The vendor decides which member of the union carries the credentials — the
/// two are one configuration, not two independent members — so a mismatch is
/// refused rather than stored as a provider whose vendor and credentials
/// disagree.
fn vendor_configuration_mismatch(vendor: &str, required_member: &str) -> ValidationException {
    field_validation_failed(
        PROVIDER_CONFIGURATION_INPUT_MEMBER,
        &format!("must carry {required_member} for a credentialProviderVendor of {vendor}"),
    )
}

/// One payment credential provider's vendor configuration as the REQUEST asked
/// for it: the vendor's own non-secret identifiers, and what is to become of each
/// secret that vendor needs.
#[derive(Debug, Clone)]
pub enum RequestedPaymentProviderConfiguration {
    /// `CoinbaseCDP`: the Coinbase Developer Platform API key and the wallet it
    /// signs with.
    CoinbaseCdp {
        api_key_id: String,
        api_key_secret: RequestedPaymentSecret,
        wallet_secret: RequestedPaymentSecret,
    },
    /// `StripePrivy`: the Privy app and the authorization it acts under.
    StripePrivy {
        app_id: String,
        authorization_id: String,
        app_secret: RequestedPaymentSecret,
        authorization_private_key: RequestedPaymentSecret,
    },
}

impl RequestedPaymentProviderConfiguration {
    /// What the configuration a request carried asks for, or the modelled
    /// validation failure it is refused for.
    pub fn from_input(
        vendor: &str,
        region: &str,
        requested: &PaymentProviderConfigurationInput,
    ) -> Result<Self, ValidationException> {
        let required_member = if vendor == PAYMENT_VENDOR_COINBASE_CDP {
            COINBASE_CDP_CONFIGURATION_MEMBER
        } else {
            STRIPE_PRIVY_CONFIGURATION_MEMBER
        };
        match requested {
            PaymentProviderConfigurationInput::coinbaseCdpConfiguration(configuration) => {
                if required_member != COINBASE_CDP_CONFIGURATION_MEMBER {
                    return Err(vendor_configuration_mismatch(vendor, required_member));
                }
                Ok(Self::CoinbaseCdp {
                    api_key_id: configuration.api_key_id.clone(),
                    api_key_secret: RequestedPaymentSecret::from_members(
                        API_KEY_SECRET_MEMBERS,
                        region,
                        configuration.api_key_secret.as_ref(),
                        configuration.api_key_secret_source.as_ref(),
                        configuration.api_key_secret_config.as_ref(),
                    )?,
                    wallet_secret: RequestedPaymentSecret::from_members(
                        WALLET_SECRET_MEMBERS,
                        region,
                        configuration.wallet_secret.as_ref(),
                        configuration.wallet_secret_source.as_ref(),
                        configuration.wallet_secret_config.as_ref(),
                    )?,
                })
            }
            PaymentProviderConfigurationInput::stripePrivyConfiguration(configuration) => {
                if required_member != STRIPE_PRIVY_CONFIGURATION_MEMBER {
                    return Err(vendor_configuration_mismatch(vendor, required_member));
                }
                Ok(Self::StripePrivy {
                    app_id: configuration.app_id.clone(),
                    authorization_id: configuration.authorization_id.clone(),
                    app_secret: RequestedPaymentSecret::from_members(
                        APP_SECRET_MEMBERS,
                        region,
                        configuration.app_secret.as_ref(),
                        configuration.app_secret_source.as_ref(),
                        configuration.app_secret_config.as_ref(),
                    )?,
                    authorization_private_key: RequestedPaymentSecret::from_members(
                        AUTHORIZATION_PRIVATE_KEY_MEMBERS,
                        region,
                        configuration.authorization_private_key.as_ref(),
                        configuration.authorization_private_key_source.as_ref(),
                        configuration.authorization_private_key_config.as_ref(),
                    )?,
                })
            }
        }
    }

    /// Store or resolve each of this vendor's secrets against AWS Secrets
    /// Manager, and report the configuration the provider is to carry.
    ///
    /// Call it with NO state guard held — it awaits.
    pub async fn resolve(
        self,
        ctx: &::smithy_server_runtime::OperationContext<'_>,
        region: &str,
        token_vault_id: &str,
        provider_name: &str,
    ) -> Result<StoredPaymentProviderConfiguration, PaymentSecretProblem> {
        match self {
            Self::CoinbaseCdp {
                api_key_id,
                api_key_secret,
                wallet_secret,
            } => Ok(StoredPaymentProviderConfiguration::CoinbaseCdp {
                api_key_id,
                api_key_secret: api_key_secret
                    .resolve(ctx, region, token_vault_id, provider_name)
                    .await?,
                wallet_secret: wallet_secret
                    .resolve(ctx, region, token_vault_id, provider_name)
                    .await?,
            }),
            Self::StripePrivy {
                app_id,
                authorization_id,
                app_secret,
                authorization_private_key,
            } => Ok(StoredPaymentProviderConfiguration::StripePrivy {
                app_id,
                authorization_id,
                app_secret: app_secret
                    .resolve(ctx, region, token_vault_id, provider_name)
                    .await?,
                authorization_private_key: authorization_private_key
                    .resolve(ctx, region, token_vault_id, provider_name)
                    .await?,
            }),
        }
    }
}

/// One payment credential provider's vendor configuration as the vault holds it:
/// the vendor's own non-secret identifiers and, per secret that vendor needs, the
/// secret Secrets Manager holds it in.
#[derive(Debug, Clone)]
pub enum StoredPaymentProviderConfiguration {
    /// `CoinbaseCDP`: the Coinbase Developer Platform API key and the wallet it
    /// signs with.
    CoinbaseCdp {
        api_key_id: String,
        api_key_secret: StoredPaymentSecret,
        wallet_secret: StoredPaymentSecret,
    },
    /// `StripePrivy`: the Privy app and the authorization it acts under.
    StripePrivy {
        app_id: String,
        authorization_id: String,
        app_secret: StoredPaymentSecret,
        authorization_private_key: StoredPaymentSecret,
    },
}

impl StoredPaymentProviderConfiguration {
    /// The `credentialProviderVendor` this configuration belongs to.
    pub fn vendor(&self) -> &'static str {
        match self {
            Self::CoinbaseCdp { .. } => PAYMENT_VENDOR_COINBASE_CDP,
            Self::StripePrivy { .. } => PAYMENT_VENDOR_STRIPE_PRIVY,
        }
    }

    /// The configuration as every response that reports this provider carries
    /// it: the vendor's identifiers, each secret's ARN, the JSON key its value is
    /// extracted under and who manages it — and no secret value, which is what
    /// `PaymentProviderConfigurationOutput` means by "no raw secrets, only
    /// ARNs".
    pub fn output(&self) -> PaymentProviderConfigurationOutput {
        match self {
            Self::CoinbaseCdp {
                api_key_id,
                api_key_secret,
                wallet_secret,
            } => PaymentProviderConfigurationOutput::coinbaseCdpConfiguration(
                CoinbaseCdpConfigurationOutput {
                    api_key_id: api_key_id.clone(),
                    api_key_secret_arn: api_key_secret.reported(),
                    api_key_secret_json_key: api_key_secret.json_key.clone(),
                    api_key_secret_source: Some(api_key_secret.source.clone()),
                    wallet_secret_arn: wallet_secret.reported(),
                    wallet_secret_json_key: wallet_secret.json_key.clone(),
                    wallet_secret_source: Some(wallet_secret.source.clone()),
                },
            ),
            Self::StripePrivy {
                app_id,
                authorization_id,
                app_secret,
                authorization_private_key,
            } => PaymentProviderConfigurationOutput::stripePrivyConfiguration(
                StripePrivyConfigurationOutput {
                    app_id: app_id.clone(),
                    app_secret_arn: app_secret.reported(),
                    app_secret_json_key: app_secret.json_key.clone(),
                    app_secret_source: Some(app_secret.source.clone()),
                    authorization_private_key_arn: authorization_private_key.reported(),
                    authorization_private_key_json_key: authorization_private_key.json_key.clone(),
                    authorization_private_key_source: Some(
                        authorization_private_key.source.clone(),
                    ),
                    authorization_id: authorization_id.clone(),
                },
            ),
        }
    }
}

/// One payment credential provider: the name it is addressed by, the ARN minted
/// for it in the token vault, the vendor configuration the vault holds and the
/// tags the create carried.
///
/// Every read of the provider reports this record, so a create and a later read
/// cannot disagree about its ARN, its vendor or the secrets it authenticates
/// with.
#[derive(Debug, Clone)]
pub struct PaymentCredentialProviderRecord {
    pub name: String,
    pub credential_provider_arn: String,
    pub configuration: StoredPaymentProviderConfiguration,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl PaymentCredentialProviderRecord {
    /// What the create of this provider reports: the four members
    /// `CreatePaymentCredentialProviderResponse` declares, every one of them off
    /// the record just written.
    pub fn create_output(&self) -> CreatePaymentCredentialProviderOutput {
        CreatePaymentCredentialProviderOutput {
            name: self.name.clone(),
            credential_provider_vendor: self.configuration.vendor().to_string(),
            credential_provider_arn: self.credential_provider_arn.clone(),
            provider_configuration_output: self.configuration.output(),
        }
    }

    /// What a read of this provider reports.
    pub fn get_output(&self) -> GetPaymentCredentialProviderOutput {
        GetPaymentCredentialProviderOutput {
            name: self.name.clone(),
            credential_provider_arn: self.credential_provider_arn.clone(),
            credential_provider_vendor: self.configuration.vendor().to_string(),
            provider_configuration_output: self.configuration.output(),
            created_time: timestamp(self.created_at),
            last_updated_time: timestamp(self.updated_at),
            tags: self.reported_tags(),
        }
    }

    /// The tags as a response reports them: the map the create carried, or no
    /// member at all for a provider carrying none, so an untagged provider is
    /// not reported as one holding an empty tag set.
    fn reported_tags(&self) -> Option<HashMap<String, String>> {
        if self.tags.is_empty() {
            return None;
        }
        Some(
            self.tags
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        )
    }
}
