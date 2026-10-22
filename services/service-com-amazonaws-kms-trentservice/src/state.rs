// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-kms-trentservice service.
//!
//! KMS's control plane is rooted at the *customer managed key* ([`Cmk`]): every
//! operation in this slice either mints one, reports one, or moves one through its
//! key state. The records and the helpers more than one operation needs live here;
//! each operation builds its own modelled output in its own `src/ops/<op>.rs`.
//!
//! ## Key specs, key material and the ciphertext envelope
//!
//! A key's `KeySpec` decides what it can do and what it is made of, and
//! [`KeySpecProfile`] is that table: the `KeyUsage` values the spec accepts, the
//! algorithms it supports under each, and the kind of key material it holds.
//!
//! A `SYMMETRIC_DEFAULT` KMS key is "a 256-bit AES-GCM key"
//! (`smithy_query(query="doc",shape="KeySpec")`), so that is exactly what a key
//! holds: 32 bytes drawn from the OS CSPRNG, never leaving this process. `Encrypt`
//! seals the plaintext under that key with AES-256-GCM, with the canonicalized
//! encryption context as additional authenticated data, and wraps the result in a
//! self-describing envelope that names the key the material belongs to. `Decrypt`
//! is the exact inverse: the envelope says which key to open it with, and the AEAD
//! tag is what makes a ciphertext from another key — or the same ciphertext under a
//! different encryption context — fail instead of decrypting.
//!
//! The AEAD and the CSPRNG come from `ring` (`ring::aead::AES_256_GCM`,
//! `ring::rand::SystemRandom`) rather than from anything written here: this file
//! composes the envelope, it does not implement cryptography.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    KeyMetadata, MultiRegionConfiguration, MultiRegionKey, CUSTOMER_MASTER_KEY_SPEC_VALUES,
};

/// `KeyState` values, from `smithy_query(shape="KeyState")`.
pub const KEY_STATE_ENABLED: &str = "Enabled";
/// See [`KEY_STATE_ENABLED`].
pub const KEY_STATE_DISABLED: &str = "Disabled";
/// See [`KEY_STATE_ENABLED`].
pub const KEY_STATE_PENDING_DELETION: &str = "PendingDeletion";
/// The state of a key created with no key material, which waits for
/// `ImportKeyMaterial` to supply it. See [`KEY_STATE_ENABLED`].
pub const KEY_STATE_PENDING_IMPORT: &str = "PendingImport";

/// The `KeyUsageType` a cryptographic encrypt/decrypt key carries, from
/// `smithy_query(shape="KeyUsageType")`. "For encrypting, decrypting,
/// re-encrypting, and generating data keys, the KeyUsage must be ENCRYPT_DECRYPT"
/// (`smithy_query(query="doc",shape="InvalidKeyUsageException")`).
pub const KEY_USAGE_ENCRYPT_DECRYPT: &str = "ENCRYPT_DECRYPT";

/// The `KeyUsageType` of an asymmetric signing key: "For asymmetric KMS keys with
/// RSA key pairs, specify `ENCRYPT_DECRYPT` or `SIGN_VERIFY`"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")`).
pub const KEY_USAGE_SIGN_VERIFY: &str = "SIGN_VERIFY";

/// The `KeyUsageType` of an HMAC key: "For HMAC KMS keys (symmetric), specify
/// `GENERATE_VERIFY_MAC`"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")`).
pub const KEY_USAGE_GENERATE_VERIFY_MAC: &str = "GENERATE_VERIFY_MAC";

/// The `KeyUsageType` of a key-agreement key: "For asymmetric KMS keys with
/// NIST-standard elliptic curve key pairs, specify `SIGN_VERIFY` or `KEY_AGREEMENT`"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")`).
pub const KEY_USAGE_KEY_AGREEMENT: &str = "KEY_AGREEMENT";

/// The default `KeySpec`: "The default value, SYMMETRIC_DEFAULT, creates a KMS key
/// with a 256-bit AES-GCM key"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeySpec")`).
pub const KEY_SPEC_SYMMETRIC_DEFAULT: &str = "SYMMETRIC_DEFAULT";

/// The default `OriginType`: "The default is AWS_KMS, which means that KMS creates
/// the key material"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Origin")`).
pub const ORIGIN_AWS_KMS: &str = "AWS_KMS";

/// The `OriginType` of a key created with no key material: "To create a KMS key with
/// no key material (for imported key material), set this value to EXTERNAL"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Origin")`).
pub const ORIGIN_EXTERNAL: &str = "EXTERNAL";

/// The `KeyManagerType` of a key the customer created, as opposed to an
/// AWS managed key, from `smithy_query(shape="KeyManagerType")`.
pub const KEY_MANAGER_CUSTOMER: &str = "CUSTOMER";

/// The only `EncryptionAlgorithmSpec` a symmetric encryption key supports:
/// "The default value, SYMMETRIC_DEFAULT, is the algorithm used for symmetric
/// encryption KMS keys"
/// (`smithy_query(query="doc",shape="EncryptRequest",member="EncryptionAlgorithm")`).
pub const ENCRYPTION_ALGORITHM_SYMMETRIC_DEFAULT: &str = "SYMMETRIC_DEFAULT";

/// The `MultiRegionKeyType` of the key a `MultiRegion` create mints, from
/// `smithy_query(shape="MultiRegionKeyType")`.
pub const MULTI_REGION_KEY_TYPE_PRIMARY: &str = "PRIMARY";

/// The Region an unsigned request is attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace and endpoint prefix of this service
/// (`smithy_query(query="service_info")` -> `awsService.arnNamespace`).
pub const ARN_SERVICE: &str = "kms";

/// The prefix of an alias name: "alias/<name>".
pub const ALIAS_PREFIX: &str = "alias/";

/// The alias-name prefix a caller may not create under: "The alias name cannot
/// begin with alias/aws/. The alias/aws/ prefix is reserved for Amazon Web Services
/// managed keys"
/// (`smithy_query(query="doc",shape="CreateAliasRequest",member="AliasName")`).
pub const ALIAS_PREFIX_AWS_MANAGED: &str = "alias/aws/";

/// The largest number of aliases one KMS key carries: KMS's published resource
/// quota "Aliases per KMS key: 50" (Quotas in the Key Management Service Developer
/// Guide, which the modelled `LimitExceededException` — "a length constraint or
/// quota was exceeded" — points at). It is also what bounds how far one key can
/// grow this store.
pub const MAX_ALIASES_PER_KEY: usize = 50;

/// Per-service state for com-amazonaws-kms-trentservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKmsTrentserviceState {
    /// Every persisted key, keyed by its minted key id (a UUID).
    pub keys: HashMap<String, Cmk>,
    /// The key ids in creation order. `ListKeys` pages over this, so a page
    /// boundary is stable across calls and a new key never reshuffles earlier
    /// pages.
    pub key_order: Vec<String>,
    /// Every persisted alias, keyed by its ALIAS ARN
    /// (`arn:aws:kms:<region>:<account>:alias/<name>`) rather than by its name:
    /// "The alias must be unique in the account and Region, but you can have
    /// aliases with the same name in different Regions"
    /// (`smithy_query(query="doc",shape="CreateAlias")`), so the name alone does
    /// not identify one. `CreateAlias` fills this, and the resolver consults it, so
    /// every operation taking a `KeyId` accepts an alias that exists.
    pub aliases: HashMap<String, Alias>,
    /// The alias ARNs in creation order, so an alias reader reports them in a
    /// stable order rather than the hash map's.
    pub alias_order: Vec<String>,
}

impl ComAmazonawsKmsTrentserviceState {
    /// Persist a freshly minted key, remembering its place in creation order.
    ///
    /// The caller refuses the request past [`MAX_KEYS`] BEFORE minting, so this
    /// never grows the store without a bound.
    pub fn insert_key(&mut self, key: Cmk) {
        self.key_order.push(key.key_id.clone());
        self.keys.insert(key.key_id.clone(), key);
    }

    /// Whether the store can hold another key, which is what `CreateKey` checks
    /// before it mints one.
    pub fn has_room_for_another_key(&self) -> bool {
        self.keys.len() < MAX_KEYS
    }

    /// Persist a freshly created alias, remembering its place in creation order.
    ///
    /// The caller refuses the request past [`MAX_ALIASES_PER_KEY`] BEFORE creating
    /// it, so this never grows the store without a bound.
    pub fn insert_alias(&mut self, alias: Alias) {
        self.alias_order.push(alias.arn.clone());
        self.aliases.insert(alias.arn.clone(), alias);
    }

    /// The alias `region`'s endpoint serves under the name `alias_name`
    /// (`alias/<name>`), which is the uniqueness `CreateAlias` enforces: an alias
    /// is unique per account and Region, so the same name in another Region is a
    /// different (absent) alias.
    pub fn alias(&self, region: &str, alias_name: &str) -> Option<&Alias> {
        self.aliases.get(&alias_arn(region, alias_name))
    }

    /// How many aliases `region`'s endpoint already holds for the key `key_id` —
    /// what bounds a key's aliases at [`MAX_ALIASES_PER_KEY`].
    pub fn alias_count_for_key(&self, region: &str, key_id: &str) -> usize {
        self.aliases
            .values()
            .filter(|alias| alias.region == region && alias.target_key_id == key_id)
            .count()
    }

    /// The key a `KeyId` request member names, accepting every form KMS does:
    /// a raw key id, a key ARN, `alias/<name>` and an alias ARN.
    ///
    /// `region` is the Region the request was signed for. A KMS key is a REGIONAL
    /// resource, so a key created through one Region's endpoint is not visible
    /// through another's.
    pub fn resolve(&self, region: &str, id: &str) -> KeyRef<'_> {
        match parse_key_identifier(id) {
            Err(()) => KeyRef::MalformedArn,
            Ok(identifier) => match self.lookup(region, &identifier) {
                Some(key) => KeyRef::Found(key),
                None => KeyRef::NotFound,
            },
        }
    }

    /// [`Self::resolve`], for an operation that then mutates the key.
    pub fn resolve_mut(&mut self, region: &str, id: &str) -> KeyRefMut<'_> {
        match parse_key_identifier(id) {
            Err(()) => KeyRefMut::MalformedArn,
            Ok(identifier) => {
                match self
                    .lookup(region, &identifier)
                    .map(|key| key.key_id.clone())
                {
                    Some(key_id) => match self.keys.get_mut(&key_id) {
                        Some(key) => KeyRefMut::Found(key),
                        None => KeyRefMut::NotFound,
                    },
                    None => KeyRefMut::NotFound,
                }
            }
        }
    }

    /// The key `region`'s endpoint serves for the exact key id `key_id` — the
    /// lookup a ciphertext envelope needs, which names a key id and nothing else.
    pub fn key_in(&self, region: &str, key_id: &str) -> Option<&Cmk> {
        self.keys.get(key_id).filter(|key| key.region == region)
    }

    /// The key ids `region`'s endpoint serves, in creation order.
    pub fn key_ids_in(&self, region: &str) -> Vec<&str> {
        self.key_order
            .iter()
            .filter_map(|key_id| self.key_in(region, key_id).map(|key| key.key_id.as_str()))
            .collect()
    }

    /// The key an already-parsed identifier names.
    fn lookup(&self, region: &str, identifier: &KeyIdentifier) -> Option<&Cmk> {
        let key = match identifier {
            KeyIdentifier::KeyId { key_id, .. } => self.key_in(region, key_id.as_str()),
            KeyIdentifier::Alias { alias, .. } => self
                .alias(region, alias.as_str())
                .and_then(|alias| self.key_in(region, &alias.target_key_id)),
        }?;
        // A key ARN addresses one key in one Region of one account: an ARN whose
        // Region or account segment names another endpoint's key is not found
        // here, even when its resource part happens to match.
        match identifier.scope() {
            Some((arn_region, account))
                if arn_region != key.region || account != key.aws_account_id =>
            {
                None
            }
            _ => Some(key),
        }
    }
}

/// What [`ComAmazonawsKmsTrentserviceState::resolve`] found. The three cases map to
/// different modelled errors, and not every operation models all of them, so the
/// mapping is the caller's.
pub enum KeyRef<'a> {
    /// The identifier named this key.
    Found(&'a Cmk),
    /// The identifier was well formed but names no key in this account and Region.
    NotFound,
    /// The identifier is an ARN that is not a well-formed KMS key or alias ARN.
    MalformedArn,
}

/// [`KeyRef`], for a mutating operation.
pub enum KeyRefMut<'a> {
    /// The identifier named this key.
    Found(&'a mut Cmk),
    /// The identifier was well formed but names no key in this account and Region.
    NotFound,
    /// The identifier is an ARN that is not a well-formed KMS key or alias ARN.
    MalformedArn,
}

/// A parsed `KeyId` request member.
enum KeyIdentifier {
    /// A raw key id, or the `key/<id>` resource of a key ARN.
    KeyId {
        key_id: String,
        /// The ARN's `(region, account)`, when the identifier was an ARN.
        scope: Option<(String, String)>,
    },
    /// `alias/<name>`, raw or as the resource of an alias ARN.
    Alias {
        alias: String,
        scope: Option<(String, String)>,
    },
}

impl KeyIdentifier {
    /// The `(region, account)` an ARN form pins the lookup to.
    fn scope(&self) -> Option<(&str, &str)> {
        let scope = match self {
            Self::KeyId { scope, .. } | Self::Alias { scope, .. } => scope.as_ref()?,
        };
        Some((scope.0.as_str(), scope.1.as_str()))
    }
}

/// Parse a `KeyId` member into the thing it names, or `Err(())` when it is an ARN
/// that is not a KMS key or alias ARN.
fn parse_key_identifier(id: &str) -> Result<KeyIdentifier, ()> {
    let Some(rest) = id.strip_prefix("arn:") else {
        return Ok(if let Some(name) = id.strip_prefix(ALIAS_PREFIX) {
            KeyIdentifier::Alias {
                alias: format!("{ALIAS_PREFIX}{name}"),
                scope: None,
            }
        } else {
            KeyIdentifier::KeyId {
                key_id: id.to_string(),
                scope: None,
            }
        });
    };

    // arn:<partition>:kms:<region>:<account>:<resource>, where the resource part
    // may itself contain ':' — hence splitn(5, ..).
    let segments: Vec<&str> = rest.splitn(5, ':').collect();
    let [partition, service, region, account, resource] = segments.as_slice() else {
        return Err(());
    };
    if partition.is_empty() || *service != ARN_SERVICE || region.is_empty() || account.is_empty() {
        return Err(());
    }
    let scope = Some(((*region).to_string(), (*account).to_string()));
    if let Some(name) = resource.strip_prefix(ALIAS_PREFIX) {
        if name.is_empty() {
            return Err(());
        }
        return Ok(KeyIdentifier::Alias {
            alias: format!("{ALIAS_PREFIX}{name}"),
            scope,
        });
    }
    match resource.strip_prefix("key/") {
        Some(key_id) if !key_id.is_empty() => Ok(KeyIdentifier::KeyId {
            key_id: key_id.to_string(),
            scope,
        }),
        _ => Err(()),
    }
}

/// How a failed `KeyId` lookup is reported: by the ARN of the resource the REQUEST
/// named.
///
/// A `KeyId` member accepts two kinds of thing — a key (a key id or a key ARN) and an
/// alias (`alias/<name>` or an alias ARN) — and KMS reports a lookup that found
/// nothing as a failure of the kind that was asked for, naming it by its ARN: a key
/// id becomes the key ARN of this endpoint, an alias name the alias ARN of this
/// endpoint, and an identifier that was already an ARN is reported as given. So an
/// alias that resolves to no key is reported as a missing ALIAS, never as a missing
/// key identifier.
///
/// BEFORE that, the Region: a key or alias ARN pins the request to one Region, so an
/// ARN naming a Region other than the one the request was signed for can never be
/// served by this endpoint at all, and KMS answers by naming the ARN's OWN Region
/// rather than a resource of the endpoint's. The recording this is read from is
/// `kms-key-description-update-and-cancel-deletion#task#5` of the
/// `simparity-wire-aws-v1` wire corpus: `UpdateKeyDescription` with
/// `KeyId = arn:aws:kms:us-east-1:<account>:key/<id>` sent to the **us-west-2**
/// endpoint answers HTTP 400 `NotFoundException: Invalid arn us-east-1` — `us-east-1`
/// is the Region in the ARN, not the Region of the endpoint, which is the one case
/// that tells the two apart.
///
/// `region` is the Region the request was signed for, and the Region of the ARN this
/// builds, since a KMS key is a regional resource and the same key id in another
/// Region is a different (absent) key. The exact sentences are examples of KMS's;
/// what this function pins is WHICH resource, in which form, the message names.
///
/// An identifier that is an ARN but not a KMS key or alias ARN has no resource ARN to
/// name, so it keeps the identifier-shaped wording. The operations that model
/// `InvalidArnException` answer that instead and never reach this case; `Encrypt` and
/// `Decrypt`, which do not model it, report such an identifier as not found.
pub fn key_not_found_message(region: &str, requested: &str) -> String {
    if let Some(arn_region) = arn_region(requested) {
        if arn_region != region {
            return format!("Invalid arn {arn_region}");
        }
    }
    match parse_key_identifier(requested) {
        Ok(KeyIdentifier::Alias { alias, scope }) => {
            format!("Alias {} is not found.", arn_of(region, scope, requested, &alias))
        }
        Ok(KeyIdentifier::KeyId { key_id, scope }) => {
            let resource = format!("key/{key_id}");
            format!(
                "Key '{}' does not exist",
                arn_of(region, scope, requested, &resource)
            )
        }
        Err(()) => format!("KMS cannot find the key with identifier {requested} in {region}."),
    }
}

/// The Region segment of `identifier`, when it is a KMS key or alias ARN.
fn arn_region(identifier: &str) -> Option<String> {
    let parsed = parse_key_identifier(identifier).ok()?;
    let (arn_region, _account) = parsed.scope()?;
    Some(arn_region.to_string())
}

/// The ARN a resolved identifier names: the one the request gave when it gave an ARN
/// (which is what `scope` records), otherwise the ARN `resource` denotes in `region`.
fn arn_of(
    region: &str,
    scope: Option<(String, String)>,
    requested: &str,
    resource: &str,
) -> String {
    if scope.is_some() {
        requested.to_string()
    } else {
        ::engine::emulator_arn(ARN_SERVICE, region, resource)
    }
}

/// The ARN of the alias `alias_name` (`alias/<name>`) in `region`, which is what
/// identifies an alias: "you can have aliases with the same name in different
/// Regions" (`smithy_query(query="doc",shape="CreateAlias")`).
pub fn alias_arn(region: &str, alias_name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, alias_name)
}

/// Whether a `KeyId`-shaped identifier names an ALIAS (`alias/<name>` or an alias
/// ARN) rather than a key.
///
/// `CreateAlias` and `UpdateAlias` take a target that must be a key — "Specify the
/// key ID or key ARN of the KMS key"
/// (`smithy_query(query="doc",shape="CreateAliasRequest",member="TargetKeyId")`) —
/// while every other `KeyId` member accepts either.
pub fn names_an_alias(identifier: &str) -> bool {
    matches!(
        parse_key_identifier(identifier),
        Ok(KeyIdentifier::Alias { .. })
    )
}

/// Whether `alias_name` is a name a caller may create, and why not when it is not.
///
/// Two rules, both from the member's own documentation
/// (`smithy_query(query="doc",shape="CreateAliasRequest",member="AliasName")`):
/// the value "must begin with alias/ followed by a name", so the prefix is required
/// and what follows it may not be empty; and "the alias name cannot begin with
/// alias/aws/", which is reserved for AWS managed keys. The character set and the
/// 1..256 length are modelled, so the boundary validator has already refused
/// anything outside them before this is reached.
///
/// One function for the whole service, so no two alias operations judge the same
/// name differently. The sentences are examples of KMS's; what this pins is WHICH
/// names are refused, and the refusal names the value the request gave.
pub fn validate_alias_name(alias_name: &str) -> Result<(), String> {
    let Some(name) = alias_name.strip_prefix(ALIAS_PREFIX) else {
        return Err(format!(
            "Alias must start with the prefix \"{ALIAS_PREFIX}\". Found: {alias_name}"
        ));
    };
    if name.is_empty() {
        return Err(format!(
            "Alias must have a name after the prefix \"{ALIAS_PREFIX}\". Found: {alias_name}"
        ));
    }
    if alias_name.starts_with(ALIAS_PREFIX_AWS_MANAGED) {
        return Err(format!(
            "Aliases beginning with \"{ALIAS_PREFIX_AWS_MANAGED}\" are reserved for Amazon Web \
             Services managed keys. Found: {alias_name}"
        ));
    }
    Ok(())
}

/// One alias: a friendly name for a KMS key, in one Region of one account.
///
/// The record holds the TARGET KEY ID, never another alias: an alias "is associated
/// with only one KMS key at a time"
/// (`smithy_query(query="doc",shape="CreateAlias")`), so resolving one is a single
/// step and can never chain.
#[derive(Debug, Clone)]
pub struct Alias {
    /// `alias/<name>`, as the request gave it.
    pub alias_name: String,
    /// `arn:aws:kms:<region>:<account>:alias/<name>`, which is also this record's
    /// map key.
    pub arn: String,
    /// The Region whose endpoint created it; an alias is a regional resource.
    pub region: String,
    /// The key id (never an alias) this alias resolves to.
    pub target_key_id: String,
    pub creation_date: SystemTime,
}

impl Alias {
    /// A freshly created alias for `target_key_id`.
    pub fn new(region: String, alias_name: String, target_key_id: String) -> Self {
        let arn = alias_arn(&region, &alias_name);
        Self {
            alias_name,
            arn,
            region,
            target_key_id,
            creation_date: SystemTime::now(),
        }
    }
}

/// One customer managed KMS key: its metadata, and the key material behind it.
///
/// `Debug` is written by hand rather than derived so the key material is REDACTED:
/// a derived one would print the 32 secret bytes into any `tracing` line or panic
/// message that formats a key record.
pub struct Cmk {
    /// The minted key id (a UUID), which is also this record's map key.
    pub key_id: String,
    /// `arn:aws:kms:<region>:<account>:key/<key-id>`.
    pub arn: String,
    pub aws_account_id: String,
    pub region: String,
    pub creation_date: SystemTime,
    pub enabled: bool,
    /// One of [`KEY_STATE_ENABLED`], [`KEY_STATE_DISABLED`],
    /// [`KEY_STATE_PENDING_DELETION`].
    pub key_state: String,
    pub description: String,
    pub key_usage: String,
    pub key_spec: String,
    pub origin: String,
    pub key_manager: String,
    pub multi_region: bool,
    /// The key policy document, as supplied or as defaulted by
    /// [`default_key_policy`].
    pub policy: String,
    /// Set by `ScheduleKeyDeletion`, cleared while the key is not pending deletion.
    pub deletion_date: Option<SystemTime>,
    /// The waiting period `ScheduleKeyDeletion` was given.
    pub pending_window_in_days: Option<i32>,
    pub tags: BTreeMap<String, String>,
    /// The key material behind this KMS key, in the form its [`Self::key_spec`]
    /// calls for. Never serialized into a response: the material of a symmetric key
    /// "never leaves KMS unencrypted"
    /// (`smithy_query(query="doc",shape="CreateKey")`).
    pub material: KeyMaterial,
    /// `KeyMaterialId`: 64 lowercase hex characters
    /// (`smithy_query(shape="BackingKeyIdType")`), derived from the material so it
    /// is stable for as long as the material is. `None` for a key whose material
    /// KMS gives no id — see [`KeyMaterial::id`].
    pub current_key_material_id: Option<String>,
}

impl std::fmt::Debug for Cmk {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Cmk")
            .field("key_id", &self.key_id)
            .field("arn", &self.arn)
            .field("aws_account_id", &self.aws_account_id)
            .field("region", &self.region)
            .field("creation_date", &self.creation_date)
            .field("enabled", &self.enabled)
            .field("key_state", &self.key_state)
            .field("description", &self.description)
            .field("key_usage", &self.key_usage)
            .field("key_spec", &self.key_spec)
            .field("origin", &self.origin)
            .field("key_manager", &self.key_manager)
            .field("multi_region", &self.multi_region)
            .field("policy", &self.policy)
            .field("deletion_date", &self.deletion_date)
            .field("pending_window_in_days", &self.pending_window_in_days)
            .field("tags", &self.tags)
            // The material itself never appears; its id identifies it without
            // disclosing it.
            .field("material", &"<redacted>")
            .field("current_key_material_id", &self.current_key_material_id)
            .finish()
    }
}

impl Cmk {
    /// Mint a key in `region` from `material`.
    ///
    /// A key KMS generated material for starts `Enabled`; a key created with no key
    /// material waits in `PendingImport` until `ImportKeyMaterial` supplies it, and
    /// is not enabled, since "without its key material, the KMS key is unusable"
    /// (`smithy_query(query="doc",shape="ImportKeyMaterialRequest",member="ValidTo")`).
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        key_id: String,
        region: String,
        description: String,
        key_usage: String,
        key_spec: String,
        origin: String,
        multi_region: bool,
        policy: String,
        tags: BTreeMap<String, String>,
        material: KeyMaterial,
    ) -> Self {
        let arn = ::engine::emulator_arn(ARN_SERVICE, &region, &format!("key/{key_id}"));
        let awaiting_import = matches!(material, KeyMaterial::AwaitingImport);
        Self {
            key_id,
            arn,
            aws_account_id: ::engine::emulator_account_id().to_string(),
            region,
            creation_date: SystemTime::now(),
            enabled: !awaiting_import,
            key_state: if awaiting_import {
                KEY_STATE_PENDING_IMPORT.to_string()
            } else {
                KEY_STATE_ENABLED.to_string()
            },
            description,
            key_usage,
            key_spec,
            origin,
            key_manager: KEY_MANAGER_CUSTOMER.to_string(),
            multi_region,
            policy,
            deletion_date: None,
            pending_window_in_days: None,
            tags,
            current_key_material_id: material.id(),
            material,
        }
    }

    /// This key's `KeyMetadata`, computed from the record — the one place the
    /// metadata is built, so `CreateKey` and `DescribeKey` cannot disagree.
    pub fn metadata(&self) -> KeyMetadata {
        KeyMetadata {
            aws_account_id: Some(self.aws_account_id.clone()),
            key_id: self.key_id.clone(),
            arn: Some(self.arn.clone()),
            creation_date: Some(date_time(self.creation_date)),
            enabled: self.enabled,
            description: Some(self.description.clone()),
            key_usage: Some(self.key_usage.clone()),
            key_state: Some(self.key_state.clone()),
            deletion_date: self.deletion_date.map(date_time),
            // Only a key with imported material expires, and importing is not
            // implemented, so no key here has a `ValidTo` or an expiration model.
            valid_to: None,
            origin: Some(self.origin.clone()),
            custom_key_store_id: None,
            cloud_hsm_cluster_id: None,
            expiration_model: None,
            key_manager: Some(self.key_manager.clone()),
            // "The KeySpec and CustomerMasterKeySpec fields have the same value"
            // (`smithy_query(shape="KeyMetadata")` -> `CustomerMasterKeySpec`) — for
            // every spec the DEPRECATED enum can express. `CustomerMasterKeySpec`
            // was frozen before the newer specs were added
            // (`smithy_query(shape="CustomerMasterKeySpec")` lists its values), so a
            // key whose spec it cannot name reports the spec through `KeySpec` only
            // rather than putting an unmodelled value on the wire.
            customer_master_key_spec: CUSTOMER_MASTER_KEY_SPEC_VALUES
                .contains(&self.key_spec.as_str())
                .then(|| self.key_spec.clone()),
            key_spec: Some(self.key_spec.clone()),
            // Each of these four lists "is present only when the KeyUsage of the
            // KMS key is" the matching one (`smithy_query(shape="KeyMetadata")`),
            // and which algorithms it names follows from the key's spec.
            encryption_algorithms: self.algorithms_under(KEY_USAGE_ENCRYPT_DECRYPT),
            signing_algorithms: self.algorithms_under(KEY_USAGE_SIGN_VERIFY),
            key_agreement_algorithms: self.algorithms_under(KEY_USAGE_KEY_AGREEMENT),
            multi_region: Some(self.multi_region),
            multi_region_configuration: self.multi_region.then(|| MultiRegionConfiguration {
                multi_region_key_type: Some(MULTI_REGION_KEY_TYPE_PRIMARY.to_string()),
                primary_key: Some(MultiRegionKey {
                    arn: Some(self.arn.clone()),
                    region: Some(self.region.clone()),
                }),
                // `ReplicateKey` is not implemented, so a primary key has no
                // replicas to report.
                replica_keys: Some(Vec::new()),
            }),
            pending_deletion_window_in_days: None,
            mac_algorithms: self.algorithms_under(KEY_USAGE_GENERATE_VERIFY_MAC),
            xks_key_configuration: None,
            current_key_material_id: self.current_key_material_id.clone(),
        }
    }

    /// The algorithms this key supports under `key_usage`, or `None` when that is
    /// not the usage the key was created with — which is what makes `KeyMetadata`
    /// carry exactly one of the four per-usage algorithm lists.
    fn algorithms_under(&self, key_usage: &str) -> Option<Vec<String>> {
        if self.key_usage != key_usage {
            return None;
        }
        let algorithms = key_spec_profile(&self.key_spec)?.algorithms_under(key_usage)?;
        Some(algorithms.iter().map(|name| (*name).to_string()).collect())
    }

    /// Whether this key can be used in a cryptographic operation right now.
    pub fn is_usable(&self) -> bool {
        self.enabled && self.key_state == KEY_STATE_ENABLED
    }

    /// Whether this key's key state is one of the two `EnableKey` and `DisableKey`
    /// move between. A key awaiting deletion or awaiting imported key material is in
    /// neither, so those operations refuse it rather than dropping the state it is
    /// actually in.
    pub fn is_enableable(&self) -> bool {
        self.key_state == KEY_STATE_ENABLED || self.key_state == KEY_STATE_DISABLED
    }
}

// ---------------------------------------------------------------------------
// Key specs: what a key of each spec can do, and the material behind it
// ---------------------------------------------------------------------------

/// The length of a `SYMMETRIC_DEFAULT` key's material: a 256-bit AES-GCM key.
pub const MATERIAL_LEN: usize = 32;

/// What one `KeySpec` is: which `KeyUsage` values a key of that spec may carry,
/// which algorithms it supports under each of them, and what its key material is.
///
/// "The `KeySpec` determines whether the KMS key contains a symmetric key or an
/// asymmetric key pair. It also determines the algorithms that the KMS key supports"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeySpec")`), so this
/// one table is what `CreateKey` validates against and what `KeyMetadata` projects
/// its algorithm lists from. A usage is valid for a spec exactly when the spec has
/// algorithms under it, which is the matrix
/// `smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")` states
/// ("For HMAC KMS keys (symmetric), specify `GENERATE_VERIFY_MAC`", "For asymmetric
/// KMS keys with RSA key pairs, specify `ENCRYPT_DECRYPT` or `SIGN_VERIFY`", and so
/// on). The algorithm names are the `EncryptionAlgorithmSpec`,
/// `SigningAlgorithmSpec`, `MacAlgorithmSpec` and `KeyAgreementAlgorithmSpec` enum
/// values (`crate::ops::types` renders each as a `*_VALUES` list); which of them
/// each spec supports is the "Key spec reference" the `KeySpec` doc links.
#[derive(Debug, Clone, Copy)]
pub struct KeySpecProfile {
    /// The `KeySpec` this profile describes.
    pub key_spec: &'static str,
    /// `EncryptionAlgorithms`, for a key of this spec whose usage is
    /// `ENCRYPT_DECRYPT`. Empty when the spec cannot encrypt.
    pub encryption_algorithms: &'static [&'static str],
    /// `SigningAlgorithms`, for usage `SIGN_VERIFY`.
    pub signing_algorithms: &'static [&'static str],
    /// `MacAlgorithms`, for usage `GENERATE_VERIFY_MAC`.
    pub mac_algorithms: &'static [&'static str],
    /// `KeyAgreementAlgorithms`, for usage `KEY_AGREEMENT`.
    pub key_agreement_algorithms: &'static [&'static str],
    /// The key material a key of this spec is minted with.
    pub material: MaterialKind,
}

impl KeySpecProfile {
    /// The algorithms a key of this spec supports under `key_usage`, or `None` when
    /// the spec does not support that usage at all.
    pub fn algorithms_under(&self, key_usage: &str) -> Option<&'static [&'static str]> {
        let algorithms = match key_usage {
            KEY_USAGE_ENCRYPT_DECRYPT => self.encryption_algorithms,
            KEY_USAGE_SIGN_VERIFY => self.signing_algorithms,
            KEY_USAGE_GENERATE_VERIFY_MAC => self.mac_algorithms,
            KEY_USAGE_KEY_AGREEMENT => self.key_agreement_algorithms,
            _ => &[],
        };
        (!algorithms.is_empty()).then_some(algorithms)
    }

    /// Whether a key of this spec may carry `key_usage`.
    pub fn supports(&self, key_usage: &str) -> bool {
        self.algorithms_under(key_usage).is_some()
    }

    /// Every `KeyUsage` this spec accepts, in the order
    /// `smithy_query(shape="KeyUsageType")` lists them — what a refusal names so the
    /// caller can pick one.
    pub fn supported_usages(&self) -> Vec<&'static str> {
        [
            KEY_USAGE_SIGN_VERIFY,
            KEY_USAGE_ENCRYPT_DECRYPT,
            KEY_USAGE_GENERATE_VERIFY_MAC,
            KEY_USAGE_KEY_AGREEMENT,
        ]
        .into_iter()
        .filter(|usage| self.supports(usage))
        .collect()
    }
}

/// What kind of key material a key of a given spec holds.
#[derive(Debug, Clone, Copy)]
pub enum MaterialKind {
    /// A 256-bit AES-GCM key, which is what `SYMMETRIC_DEFAULT` names: "a 256-bit
    /// AES-GCM key that is used for encryption and decryption"
    /// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeySpec")`).
    Aes256,
    /// An HMAC key of this many bytes: an `HMAC_<bits>` spec names the length of its
    /// key in bits, so `HMAC_256` is 32 bytes.
    Hmac(usize),
    /// An asymmetric key pair. This slice does not generate asymmetric key material
    /// (see [`KeyMaterial::KeyPairNotGenerated`]).
    KeyPair,
}

/// The key material behind one KMS key.
pub enum KeyMaterial {
    /// The 256-bit AES-GCM key of a symmetric encryption KMS key, from the OS
    /// CSPRNG. `Encrypt` and `Decrypt` seal and open under exactly these bytes.
    Aes256([u8; MATERIAL_LEN]),
    /// The key of an HMAC KMS key: as many bytes from the OS CSPRNG as the spec's
    /// name gives bits.
    Hmac(Vec<u8>),
    /// A key pair this emulator has not generated.
    ///
    /// The KMS key itself is real: it is persisted, reported by `DescribeKey` and
    /// `ListKeys`, and moves through its key states like any other. What is absent
    /// is the private key, so every operation that would USE it — `Sign`, `Verify`,
    /// `GetPublicKey`, `DeriveSharedSecret`, and `Encrypt`/`Decrypt` for an
    /// asymmetric spec — answers the scaffold's 501 rather than a fabricated
    /// signature, public key or ciphertext.
    KeyPairNotGenerated,
    /// No key material, because the caller asked for a key with none: "To create a
    /// KMS key with no key material (for imported key material), set this value to
    /// EXTERNAL"
    /// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Origin")`). Such
    /// a key waits in `PendingImport` until `ImportKeyMaterial` supplies material.
    AwaitingImport,
}

impl KeyMaterial {
    /// The `CurrentKeyMaterialId` of this material, which "is present for symmetric
    /// encryption keys with `AWS_KMS` or `EXTERNAL` origin"
    /// (`smithy_query(shape="KeyMetadata")` -> `CurrentKeyMaterialId`) and is
    /// therefore absent for every other kind of material.
    ///
    /// It is the 64 lowercase hex characters `BackingKeyIdType` requires, derived
    /// from the material itself so two keys never share one and a read never
    /// changes it.
    fn id(&self) -> Option<String> {
        let Self::Aes256(material) = self else {
            return None;
        };
        let mut hasher = Sha256::new();
        hasher.update(b"kms-key-material-id\0");
        hasher.update(material);
        Some(hex::encode(hasher.finalize()))
    }

    /// The AES-256-GCM key behind a symmetric encryption KMS key, or `None` for
    /// material of any other kind.
    pub fn aes256(&self) -> Option<&[u8; MATERIAL_LEN]> {
        match self {
            Self::Aes256(material) => Some(material),
            Self::Hmac(_) | Self::KeyPairNotGenerated | Self::AwaitingImport => None,
        }
    }
}

/// The key material a key of `kind` is minted with.
///
/// The kinds that HAVE material draw it from the OS CSPRNG; `MaterialKind::KeyPair`
/// mints none, because this emulator does not generate asymmetric key pairs, and
/// says so in the material it returns rather than inventing bytes that are not a key
/// pair.
///
/// `None` means the platform refused to produce entropy; the caller answers a
/// modelled server error rather than minting a key with predictable material.
pub fn mint_material(kind: MaterialKind) -> Option<KeyMaterial> {
    match kind {
        MaterialKind::Aes256 => {
            let mut material = [0u8; MATERIAL_LEN];
            SystemRandom::new().fill(&mut material).ok()?;
            Some(KeyMaterial::Aes256(material))
        }
        MaterialKind::Hmac(len) => {
            let mut material = vec![0u8; len];
            SystemRandom::new().fill(&mut material).ok()?;
            Some(KeyMaterial::Hmac(material))
        }
        MaterialKind::KeyPair => Some(KeyMaterial::KeyPairNotGenerated),
    }
}

/// The `SigningAlgorithmSpec` values an RSA key pair supports: RSASSA-PSS and
/// RSASSA-PKCS1-v1_5 over each SHA-2 digest. "When signing with RSA key pairs,
/// RSASSA-PSS algorithms are preferred. We include RSASSA-PKCS1-v1_5 algorithms for
/// compatibility with existing applications"
/// (`smithy_query(query="doc",shape="SignRequest",member="SigningAlgorithm")`).
///
/// Every list in this table is in ascending order of the algorithm name, which is
/// the order KMS reports these lists in and the order the model's own rendered enum
/// values carry (`crate::ops::types::SIGNING_ALGORITHM_SPEC_VALUES`).
const RSA_SIGNING_ALGORITHMS: &[&str] = &[
    "RSASSA_PKCS1_V1_5_SHA_256",
    "RSASSA_PKCS1_V1_5_SHA_384",
    "RSASSA_PKCS1_V1_5_SHA_512",
    "RSASSA_PSS_SHA_256",
    "RSASSA_PSS_SHA_384",
    "RSASSA_PSS_SHA_512",
];

/// The `EncryptionAlgorithmSpec` values an RSA key pair supports.
const RSA_ENCRYPTION_ALGORITHMS: &[&str] = &["RSAES_OAEP_SHA_1", "RSAES_OAEP_SHA_256"];

/// The only `KeyAgreementAlgorithmSpec` (`smithy_query(shape="KeyAgreementAlgorithmSpec")`),
/// which every spec that can derive a shared secret supports.
const ECDH: &[&str] = &["ECDH"];

/// No algorithms: the spec does not support that usage.
const NONE: &[&str] = &[];

/// Every `KeySpec` `smithy_query(shape="KeySpec")` declares, with what each one can
/// do. A spec missing from this table is one this service cannot mint.
const KEY_SPEC_PROFILES: &[KeySpecProfile] = &[
    // Symmetric encryption key (default).
    KeySpecProfile {
        key_spec: KEY_SPEC_SYMMETRIC_DEFAULT,
        encryption_algorithms: &[ENCRYPTION_ALGORITHM_SYMMETRIC_DEFAULT],
        signing_algorithms: NONE,
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::Aes256,
    },
    // HMAC keys (symmetric): each spec supports the one MAC algorithm over the
    // digest its name gives.
    KeySpecProfile {
        key_spec: "HMAC_224",
        encryption_algorithms: NONE,
        signing_algorithms: NONE,
        mac_algorithms: &["HMAC_SHA_224"],
        key_agreement_algorithms: NONE,
        material: MaterialKind::Hmac(28),
    },
    KeySpecProfile {
        key_spec: "HMAC_256",
        encryption_algorithms: NONE,
        signing_algorithms: NONE,
        mac_algorithms: &["HMAC_SHA_256"],
        key_agreement_algorithms: NONE,
        material: MaterialKind::Hmac(32),
    },
    KeySpecProfile {
        key_spec: "HMAC_384",
        encryption_algorithms: NONE,
        signing_algorithms: NONE,
        mac_algorithms: &["HMAC_SHA_384"],
        key_agreement_algorithms: NONE,
        material: MaterialKind::Hmac(48),
    },
    KeySpecProfile {
        key_spec: "HMAC_512",
        encryption_algorithms: NONE,
        signing_algorithms: NONE,
        mac_algorithms: &["HMAC_SHA_512"],
        key_agreement_algorithms: NONE,
        material: MaterialKind::Hmac(64),
    },
    // Asymmetric RSA key pairs (encryption and decryption -or- signing and
    // verification).
    KeySpecProfile {
        key_spec: "RSA_2048",
        encryption_algorithms: RSA_ENCRYPTION_ALGORITHMS,
        signing_algorithms: RSA_SIGNING_ALGORITHMS,
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::KeyPair,
    },
    KeySpecProfile {
        key_spec: "RSA_3072",
        encryption_algorithms: RSA_ENCRYPTION_ALGORITHMS,
        signing_algorithms: RSA_SIGNING_ALGORITHMS,
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::KeyPair,
    },
    KeySpecProfile {
        key_spec: "RSA_4096",
        encryption_algorithms: RSA_ENCRYPTION_ALGORITHMS,
        signing_algorithms: RSA_SIGNING_ALGORITHMS,
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::KeyPair,
    },
    // Asymmetric NIST-standard elliptic curve key pairs (signing and verification
    // -or- deriving shared secrets): the ECDSA algorithm pairs with the curve's
    // digest size.
    KeySpecProfile {
        key_spec: "ECC_NIST_P256",
        encryption_algorithms: NONE,
        signing_algorithms: &["ECDSA_SHA_256"],
        mac_algorithms: NONE,
        key_agreement_algorithms: ECDH,
        material: MaterialKind::KeyPair,
    },
    KeySpecProfile {
        key_spec: "ECC_NIST_P384",
        encryption_algorithms: NONE,
        signing_algorithms: &["ECDSA_SHA_384"],
        mac_algorithms: NONE,
        key_agreement_algorithms: ECDH,
        material: MaterialKind::KeyPair,
    },
    KeySpecProfile {
        key_spec: "ECC_NIST_P521",
        encryption_algorithms: NONE,
        signing_algorithms: &["ECDSA_SHA_512"],
        mac_algorithms: NONE,
        key_agreement_algorithms: ECDH,
        material: MaterialKind::KeyPair,
    },
    // "ECC_NIST_EDWARDS25519 (ed25519) - signing and verification only", with both
    // Ed25519 signing algorithms the model declares
    // (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeySpec")`).
    KeySpecProfile {
        key_spec: "ECC_NIST_EDWARDS25519",
        encryption_algorithms: NONE,
        signing_algorithms: &["ED25519_PH_SHA_512", "ED25519_SHA_512"],
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::KeyPair,
    },
    // Other asymmetric elliptic curve key pairs (signing and verification).
    KeySpecProfile {
        key_spec: "ECC_SECG_P256K1",
        encryption_algorithms: NONE,
        signing_algorithms: &["ECDSA_SHA_256"],
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::KeyPair,
    },
    // Asymmetric ML-DSA key pairs (signing and verification), which share the one
    // ML-DSA signing algorithm the model declares.
    KeySpecProfile {
        key_spec: "ML_DSA_44",
        encryption_algorithms: NONE,
        signing_algorithms: &["ML_DSA_SHAKE_256"],
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::KeyPair,
    },
    KeySpecProfile {
        key_spec: "ML_DSA_65",
        encryption_algorithms: NONE,
        signing_algorithms: &["ML_DSA_SHAKE_256"],
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::KeyPair,
    },
    KeySpecProfile {
        key_spec: "ML_DSA_87",
        encryption_algorithms: NONE,
        signing_algorithms: &["ML_DSA_SHAKE_256"],
        mac_algorithms: NONE,
        key_agreement_algorithms: NONE,
        material: MaterialKind::KeyPair,
    },
    // "SM2 key pairs (encryption and decryption -or- signing and verification -or-
    // deriving shared secrets)".
    KeySpecProfile {
        key_spec: "SM2",
        encryption_algorithms: &["SM2PKE"],
        signing_algorithms: &["SM2DSA"],
        mac_algorithms: NONE,
        key_agreement_algorithms: ECDH,
        material: MaterialKind::KeyPair,
    },
];

/// The profile of `key_spec`, or `None` when it is not a `KeySpec` this service
/// knows — which is what makes an unknown spec a refusal rather than a key nothing
/// can use.
pub fn key_spec_profile(key_spec: &str) -> Option<&'static KeySpecProfile> {
    KEY_SPEC_PROFILES
        .iter()
        .find(|profile| profile.key_spec == key_spec)
}

/// A freshly minted key id.
///
/// A single-Region key id is a version-4 UUID. A multi-Region key id is the form
/// KMS gives one so a client can recognise it on sight: `mrk-` followed by 32
/// hexadecimal characters.
pub fn mint_key_id(multi_region: bool) -> String {
    let id = ::uuid::Uuid::new_v4();
    if multi_region {
        format!("mrk-{}", id.simple())
    } else {
        id.to_string()
    }
}

/// The largest number of keys this emulator's store holds.
///
/// A bound the EMULATOR imposes, not one KMS documents: `CreateKey` is reachable
/// by any caller, nothing in this slice ever removes a key (`ScheduleKeyDeletion`
/// only moves it to `PendingDeletion`), so without a cap a loop of valid creates
/// grows the process without end. Past it `CreateKey` refuses with the modelled
/// `LimitExceededException` — "a length constraint or quota was exceeded" — and
/// persists nothing.
pub const MAX_KEYS: usize = 500;

/// The largest number of tags one key carries: "You can assign up to 50 tags to a
/// KMS key" is KMS's own per-key tag quota, and it also bounds what one request can
/// make the store retain.
pub const MAX_TAGS_PER_KEY: usize = 50;

/// AWS's default key policy, which "gives the Amazon Web Services account that owns
/// the KMS key permission to use it in all cryptographic operations"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`). Used
/// when `CreateKey` is given no `Policy`.
pub fn default_key_policy() -> String {
    // `arn:aws:iam::<account>:root` — IAM is global, so the Region segment of the
    // principal's ARN is empty.
    let root = ::engine::emulator_arn("iam", "", "root");
    ::serde_json::json!({
        "Version": "2012-10-17",
        "Id": "key-default-1",
        "Statement": [{
            "Sid": "Enable IAM User Permissions",
            "Effect": "Allow",
            "Principal": { "AWS": root },
            "Action": "kms:*",
            "Resource": "*"
        }]
    })
    .to_string()
}

/// Render an instant the way KMS renders `CreationDate` / `DeletionDate`.
pub fn date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// How an identifier that is an ARN, but not a KMS key or alias ARN, is refused:
/// `InvalidArnException`'s message names the identifier the request gave, since that
/// string is the whole of what is wrong with it.
///
/// One function for the whole service, so no two operations word the same malformed
/// identifier differently.
pub fn invalid_arn_message(requested: &str) -> String {
    format!("The specified ARN is not valid: {requested}.")
}

/// How a refusal caused by the key's own key state is worded.
///
/// KMS names the key by its ARN and says what about its state refuses the call, so the
/// caller learns whether the state can change: the message is "<key arn> is <that key
/// state, as prose>." — recorded on the wire as "… is pending deletion." for a key
/// whose deletion is scheduled and "… is disabled." for a disabled one.
///
/// The prose comes from the `KeyState` value itself
/// (`smithy_query(query="shape",shape="KeyState")`, whose values are camel case): each
/// word is lowercased and separated by a space, so a state this slice does not reach
/// yet is worded the same way without a case for it.
///
/// One function for the whole service, so no two operations word the same key state
/// differently.
pub fn invalid_state_message(key: &Cmk) -> String {
    format!("{} is {}.", key.arn, key_state_prose(&key.key_state))
}

/// How KMS words the refusal of a request that REQUIRES a key state the key is not in
/// — the negative of [`invalid_state_message`]: where an operation refused because of
/// the state the key is in names that state, one refused because the key is outside
/// the single state it needs names the state it is missing, "<key arn> is not pending
/// deletion."
pub fn required_state_message(key: &Cmk, required_state: &str) -> String {
    format!("{} is not {}.", key.arn, key_state_prose(required_state))
}

/// A `KeyState` value as prose: `PendingDeletion` -> `pending deletion`.
fn key_state_prose(key_state: &str) -> String {
    let mut prose = String::with_capacity(key_state.len() + 2);
    for (index, c) in key_state.char_indices() {
        if c.is_ascii_uppercase() && index != 0 {
            prose.push(' ');
        }
        prose.extend(c.to_lowercase());
    }
    prose
}

/// The AWS Region a request was signed for, which is the Region the keys it creates
/// live in. Falls back to the emulator's default Region for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

// ---------------------------------------------------------------------------
// The ciphertext envelope
// ---------------------------------------------------------------------------

/// The envelope version this build writes. It is the first byte of every
/// ciphertext, so a blob from another scheme is rejected rather than mis-parsed.
const ENVELOPE_VERSION: u8 = 1;

/// AES-GCM's nonce length, as `ring::aead` defines it for `AES_256_GCM`.
const NONCE_LEN_BYTES: usize = NONCE_LEN;

/// AES-GCM's authentication tag length (`AES_256_GCM.tag_len()`).
const TAG_LEN: usize = 16;

/// A parsed ciphertext envelope: which key sealed it, and the sealed bytes.
pub struct Envelope {
    /// The key id of the KMS key whose material sealed this ciphertext.
    pub key_id: String,
    nonce: [u8; NONCE_LEN_BYTES],
    body: Vec<u8>,
}

/// This key's material as an AES-256-GCM key, or `None` when the key is not a
/// symmetric encryption key and has no AES material to seal or open with.
fn cipher(key: &Cmk) -> Option<LessSafeKey> {
    UnboundKey::new(&AES_256_GCM, key.material.aes256()?)
        .ok()
        .map(LessSafeKey::new)
}

/// Seal `plaintext` under `key`'s material, binding `context` into the ciphertext
/// as additional authenticated data.
///
/// Returns `None` only if the AEAD itself refuses the input (a plaintext larger
/// than AES-GCM can carry), which the modelled 4096-byte bound already excludes.
pub fn seal(
    key: &Cmk,
    plaintext: &[u8],
    context: Option<&HashMap<String, String>>,
) -> Option<Vec<u8>> {
    let cipher = cipher(key)?;
    let mut nonce = [0u8; NONCE_LEN_BYTES];
    SystemRandom::new().fill(&mut nonce).ok()?;
    let aad = encryption_context_aad(context);

    // `seal_in_place_append_tag` leaves `body` holding the ciphertext followed by
    // the 16-byte authentication tag.
    let mut body = plaintext.to_vec();
    cipher
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(aad.as_slice()),
            &mut body,
        )
        .ok()?;

    let id = key.key_id.as_bytes();
    let mut blob = Vec::with_capacity(2 + id.len() + NONCE_LEN_BYTES + body.len());
    blob.push(ENVELOPE_VERSION);
    blob.push(u8::try_from(id.len()).ok()?);
    blob.extend_from_slice(id);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&body);
    Some(blob)
}

/// Read the envelope a `CiphertextBlob` carries, or `None` when the bytes are not
/// one — which is what makes a foreign or truncated blob an
/// `InvalidCiphertextException` instead of a panic.
pub fn parse_envelope(blob: &[u8]) -> Option<Envelope> {
    let (&version, rest) = blob.split_first()?;
    if version != ENVELOPE_VERSION {
        return None;
    }
    let (&id_len, rest) = rest.split_first()?;
    let id_len = usize::from(id_len);
    if id_len == 0 || rest.len() < id_len + NONCE_LEN_BYTES + TAG_LEN {
        return None;
    }
    let (id, rest) = rest.split_at(id_len);
    let key_id = std::str::from_utf8(id).ok()?.to_string();
    let (nonce, body) = rest.split_at(NONCE_LEN_BYTES);
    Some(Envelope {
        key_id,
        nonce: nonce.try_into().ok()?,
        body: body.to_vec(),
    })
}

/// Open an envelope with `key`'s material under `context`.
///
/// `None` means the AEAD tag did not verify: the ciphertext was sealed by another
/// key, under a different encryption context, or has been altered.
pub fn unseal(
    key: &Cmk,
    envelope: &Envelope,
    context: Option<&HashMap<String, String>>,
) -> Option<Vec<u8>> {
    let cipher = cipher(key)?;
    let aad = encryption_context_aad(context);
    let mut body = envelope.body.clone();
    let plaintext = cipher
        .open_in_place(
            Nonce::assume_unique_for_key(envelope.nonce),
            Aad::from(aad.as_slice()),
            &mut body,
        )
        .ok()?;
    Some(plaintext.to_vec())
}

/// The encryption context as additional authenticated data.
///
/// The context is an unordered map, so it is canonicalized before it is
/// authenticated: entries sorted by key, each key and value length-prefixed so no
/// two different maps can encode to the same bytes. An absent context and an empty
/// one authenticate identically, which is what makes them interchangeable.
fn encryption_context_aad(context: Option<&HashMap<String, String>>) -> Vec<u8> {
    let mut entries: Vec<(&str, &str)> = context
        .map(|context| {
            context
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    entries.sort_unstable();

    let mut aad = Vec::new();
    for (key, value) in entries {
        for part in [key, value] {
            let len = u32::try_from(part.len()).unwrap_or(u32::MAX);
            aad.extend_from_slice(&len.to_be_bytes());
            aad.extend_from_slice(part.as_bytes());
        }
    }
    aad
}

// ---------------------------------------------------------------------------
// ListKeys pagination
// ---------------------------------------------------------------------------

/// Tags a `NextMarker` as one this service minted, and for which list.
const MARKER_PREFIX: &str = "keys/1/";

/// The `NextMarker` that resumes a list after `key_id`.
///
/// Hex-encoded so the token is opaque to a caller (who must pass it back
/// verbatim) while still satisfying `MarkerType`'s `^[\u0020-\u00FF]*$` pattern.
pub fn encode_marker(key_id: &str) -> String {
    hex::encode(format!("{MARKER_PREFIX}{key_id}"))
}

/// The key id a `Marker` resumes after, or `None` when the token is not one this
/// service minted.
pub fn decode_marker(marker: &str) -> Option<String> {
    let decoded = hex::decode(marker).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let key_id = text.strip_prefix(MARKER_PREFIX)?;
    (!key_id.is_empty()).then(|| key_id.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsKmsTrentserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsKmsTrentserviceState>)
}
