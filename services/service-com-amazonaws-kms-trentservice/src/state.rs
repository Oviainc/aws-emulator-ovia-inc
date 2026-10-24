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
//! different encryption context — fail instead of decrypting. Rotating a key's
//! material changes which version `Encrypt` seals under and retains the versions it
//! replaces, so `Decrypt` opens a ciphertext with whichever version sealed it.
//!
//! The AEAD and the CSPRNG come from `ring` (`ring::aead::AES_256_GCM`,
//! `ring::rand::SystemRandom`) rather than from anything written here: this file
//! composes the envelope, it does not implement cryptography.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::time::SystemTime;

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    AliasListEntry, GrantConstraints, GrantListEntry, KeyMetadata, MultiRegionConfiguration,
    MultiRegionKey, Tag, CUSTOMER_MASTER_KEY_SPEC_VALUES, DATA_KEY_SPEC_VALUES,
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

/// The `RotationType` of a rotation a caller asked for, as opposed to `AUTOMATIC`,
/// from `smithy_query(shape="RotationType")`.
pub const ROTATION_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// How many on-demand rotations one KMS key allows: "You can perform on-demand key
/// rotation a maximum of 25 times per KMS key"
/// (`smithy_query(query="doc",shape="RotateKeyOnDemand")`).
pub const MAX_ON_DEMAND_ROTATIONS: usize = 25;

/// The rotation period an `EnableKeyRotation` that names none gets: "If no value is
/// specified, the default value is 365 days"
/// (`smithy_query(query="doc",shape="EnableKeyRotationRequest",member="RotationPeriodInDays")`).
pub const DEFAULT_ROTATION_PERIOD_IN_DAYS: i32 = 365;

/// The `MultiRegionKeyType` of the key a `MultiRegion` create mints, from
/// `smithy_query(shape="MultiRegionKeyType")`.
pub const MULTI_REGION_KEY_TYPE_PRIMARY: &str = "PRIMARY";

/// The `MultiRegionKeyType` of a key of a multi-Region set that is not the set's
/// primary key, from `smithy_query(shape="MultiRegionKeyType")`.
pub const MULTI_REGION_KEY_TYPE_REPLICA: &str = "REPLICA";

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

/// The largest number of aliases this emulator's store holds across every key and
/// Region.
///
/// A bound the EMULATOR imposes, not a quota KMS publishes. It exists because
/// [`MAX_ALIASES_PER_KEY`] is counted per key AND Region, and the Region comes from
/// the caller's own credential scope rather than from a fixed set, so the per-key
/// count alone leaves the number of records the store holds in the caller's hands.
/// Past it `CreateAlias` refuses with the modelled `LimitExceededException` and
/// persists nothing.
pub const MAX_ALIASES: usize = 5_000;

/// [`MAX_ALIASES`], for grants.
pub const MAX_GRANTS: usize = 5_000;

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
    /// Every persisted grant, keyed by its grant id. A grant id is minted from the
    /// OS CSPRNG ([`mint_grant_id`]) and so identifies one grant across every key
    /// and Region.
    pub grants: HashMap<String, Grant>,
    /// The grant ids in creation order. A grant reader pages over this, so a page
    /// boundary is stable across calls and a new grant never reshuffles earlier
    /// pages.
    pub grant_order: Vec<String>,
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

    /// Whether the store can hold another alias, across every key and Region, which
    /// is what `CreateAlias` checks before it creates one.
    ///
    /// The per-key bound alone does not bound the STORE: the Region an alias lives in
    /// comes from the caller's own credential scope and is not drawn from a fixed set,
    /// so the per-key count is per (key, Region) and its dimension is the caller's to
    /// choose. This cap is over the whole map, so no sequence of requests can grow it
    /// past [`MAX_ALIASES`].
    pub fn has_room_for_another_alias(&self) -> bool {
        self.aliases.len() < MAX_ALIASES
    }

    /// Whether the store can hold another grant, across every key and Region, which
    /// is what `CreateGrant` checks before it mints one. See
    /// [`Self::has_room_for_another_alias`] for why the per-key bound is not enough.
    pub fn has_room_for_another_grant(&self) -> bool {
        self.grants.len() < MAX_GRANTS
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

    /// [`Self::alias`], for an operation that then repoints it at another key.
    pub fn alias_mut(&mut self, region: &str, alias_name: &str) -> Option<&mut Alias> {
        self.aliases.get_mut(&alias_arn(region, alias_name))
    }

    /// Forget the alias `region`'s endpoint serves under `alias_name`, answering the
    /// record that was dropped (`None` when there was none).
    ///
    /// Removal is from the alias store ITSELF, which is the one index every `KeyId`
    /// resolution consults ([`Self::lookup`]): an alias that is gone resolves to
    /// nothing from the next request on, rather than lingering as the last known
    /// mapping. The creation order is kept in step so a later reader cannot page over
    /// an ARN the store no longer holds.
    pub fn remove_alias(&mut self, region: &str, alias_name: &str) -> Option<Alias> {
        let arn = alias_arn(region, alias_name);
        let removed = self.aliases.remove(&arn)?;
        self.alias_order.retain(|held| *held != arn);
        Some(removed)
    }

    /// The aliases `region`'s endpoint serves, in creation order — what an alias
    /// reader pages over, so a page boundary is stable across calls.
    pub fn aliases_in(&self, region: &str) -> Vec<&Alias> {
        self.alias_order
            .iter()
            .filter_map(|arn| self.aliases.get(arn))
            .filter(|alias| alias.region == region)
            .collect()
    }

    /// How many aliases `region`'s endpoint already holds for the key `key_id` —
    /// what bounds a key's aliases at [`MAX_ALIASES_PER_KEY`].
    pub fn alias_count_for_key(&self, region: &str, key_id: &str) -> usize {
        self.aliases
            .values()
            .filter(|alias| alias.region == region && alias.target_key_id == key_id)
            .count()
    }

    /// Persist a freshly created grant, remembering its place in creation order.
    ///
    /// The caller refuses the request past [`MAX_GRANTS_PER_KEY`] BEFORE minting it,
    /// so this never grows the store without a bound.
    pub fn insert_grant(&mut self, grant: Grant) {
        self.grant_order.push(grant.grant_id.clone());
        self.grants.insert(grant.grant_id.clone(), grant);
    }

    /// The grant `region`'s endpoint serves for the exact grant id `grant_id`.
    ///
    /// A grant belongs to one key in one Region, so a grant id minted through
    /// another Region's endpoint is not found here.
    pub fn grant(&self, region: &str, grant_id: &str) -> Option<&Grant> {
        self.grants
            .get(grant_id)
            .filter(|grant| grant.region == region)
    }

    /// The grants `region`'s endpoint holds for the key `key_id`, in creation order
    /// — what a grant reader pages over, so a page boundary is stable across calls.
    pub fn grants_for_key(&self, region: &str, key_id: &str) -> Vec<&Grant> {
        self.grants_in(region)
            .into_iter()
            .filter(|grant| grant.key_id == key_id)
            .collect()
    }

    /// Every grant `region`'s endpoint holds, in creation order. A grant reader that
    /// is not scoped to one key pages over this.
    pub fn grants_in(&self, region: &str) -> Vec<&Grant> {
        self.grant_order
            .iter()
            .filter_map(|grant_id| self.grants.get(grant_id))
            .filter(|grant| grant.region == region)
            .collect()
    }

    /// The grant a grant token names, or `None` when the token is not one this
    /// service minted, or names a grant the store no longer holds because it was
    /// retired or revoked.
    pub fn grant_for_token(&self, region: &str, token: &str) -> Option<&Grant> {
        self.grant(region, &decode_grant_token(token)?)
    }

    /// The grants the grant tokens a request presents name.
    ///
    /// An empty `tokens` answers an empty list: a request that presents none is not
    /// asking to be authorized by a grant.
    ///
    /// [`GrantRefusal::MalformedToken`] when a token is not one this service minted,
    /// so it names no grant at all. [`GrantRefusal::NotAuthorized`] when every token
    /// is well formed but none names a grant the store still holds: the permission
    /// that came from a grant is gone once the grant is.
    pub fn grants_for_tokens(
        &self,
        region: &str,
        tokens: &[String],
    ) -> Result<Vec<&Grant>, GrantRefusal> {
        if tokens.is_empty() {
            return Ok(Vec::new());
        }
        let mut named = Vec::with_capacity(tokens.len());
        for token in tokens {
            let grant_id = decode_grant_token(token).ok_or(GrantRefusal::MalformedToken)?;
            if let Some(grant) = self.grant(region, &grant_id) {
                named.push(grant);
            }
        }
        if named.is_empty() {
            return Err(GrantRefusal::NotAuthorized);
        }
        Ok(named)
    }

    /// Forget the grant `grant_id`, answering the record that was dropped (`None`
    /// when `region`'s endpoint holds no such grant).
    ///
    /// Removal is from the grant store ITSELF, so the permissions the grant carried
    /// are gone from the next request on, and the creation order is kept in step so a
    /// later reader cannot page over a grant the store no longer holds.
    pub fn remove_grant(&mut self, region: &str, grant_id: &str) -> Option<Grant> {
        self.grant(region, grant_id)?;
        let removed = self.grants.remove(grant_id)?;
        self.grant_order.retain(|held| held != grant_id);
        Some(removed)
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
            // An identifier that already WAS an ARN is reported as given; a bare
            // `alias/<name>` is reported as this endpoint's alias ARN, which is what
            // `alias_not_found_message` builds.
            match scope {
                Some(_) => format!("Alias {requested} is not found."),
                None => alias_not_found_message(region, &alias),
            }
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

/// How a failed ALIAS lookup is reported: by the alias ARN of the name the request
/// named.
///
/// `DeleteAlias` and `UpdateAlias` take an `AliasName`, never a `KeyId`, so the thing
/// that was not found is always an alias of THIS endpoint — and KMS names it by its
/// ARN, exactly as [`key_not_found_message`] does for a `KeyId` that named an alias.
/// One function for both wordings, so the two can never drift apart.
pub fn alias_not_found_message(region: &str, alias_name: &str) -> String {
    format!("Alias {} is not found.", alias_arn(region, alias_name))
}

/// The ARN of the alias `alias_name` (`alias/<name>`) in `region`, which is what
/// identifies an alias: "you can have aliases with the same name in different
/// Regions" (`smithy_query(query="doc",shape="CreateAlias")`).
pub fn alias_arn(region: &str, alias_name: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, alias_name)
}

/// The ARN of the key `key_id` in `region`:
/// `arn:aws:kms:<region>:<account>:key/<key-id>`. A KMS key is a regional
/// resource, so the Region is part of the name it is known by — which is what lets
/// one key id of a multi-Region set be named per Region. One builder for every key
/// ARN, as [`alias_arn`] is for an alias.
pub fn key_arn(region: &str, key_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("key/{key_id}"))
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
    /// "Date and time that the alias was most recently associated with a KMS key in
    /// the account and Region"
    /// (`smithy_query(query="doc",shape="AliasListEntry",member="LastUpdatedDate")`):
    /// the creation instant until `UpdateAlias` associates the name with another key.
    pub last_updated_date: SystemTime,
}

impl Alias {
    /// A freshly created alias for `target_key_id`.
    pub fn new(region: String, alias_name: String, target_key_id: String) -> Self {
        let arn = alias_arn(&region, &alias_name);
        let now = SystemTime::now();
        Self {
            alias_name,
            arn,
            region,
            target_key_id,
            creation_date: now,
            last_updated_date: now,
        }
    }

    /// Associate this alias with another key: "To associate an existing alias with a
    /// different KMS key, call UpdateAlias"
    /// (`smithy_query(query="doc",shape="DeleteAlias")`). The name, the ARN and the
    /// creation date are the alias's identity and do not move; only the target and the
    /// instant it was last associated do.
    pub fn repoint(&mut self, target_key_id: String) {
        self.target_key_id = target_key_id;
        self.last_updated_date = SystemTime::now();
    }

    /// This alias as `ListAliases` reports it.
    pub fn list_entry(&self) -> AliasListEntry {
        AliasListEntry {
            alias_name: Some(self.alias_name.clone()),
            alias_arn: Some(self.arn.clone()),
            target_key_id: Some(self.target_key_id.clone()),
            creation_date: Some(date_time(self.creation_date)),
            last_updated_date: Some(date_time(self.last_updated_date)),
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
    /// Which Region of this key's multi-Region key set holds its primary key and
    /// which hold its replicas, or `None` for a single-Region key — see
    /// [`MultiRegionTopology`]. `Some` is exactly what the `MultiRegion` metadata
    /// member reports as `true`, so the two can never disagree.
    pub multi_region_topology: Option<MultiRegionTopology>,
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
    /// Every version of this key's material that a rotation has replaced, oldest
    /// first. Rotation replaces which material the key encrypts UNDER; it does not
    /// discard the material already in use, because "KMS retains all key material
    /// until you delete the KMS key"
    /// (`smithy_query(query="doc",shape="EnableKeyRotation")`), which is what keeps
    /// a ciphertext sealed before a rotation decryptable after it.
    pub superseded_material: Vec<KeyMaterial>,
    /// This key's completed rotations, oldest first — the record
    /// `ListKeyRotations` reports and the one [`Self::on_demand_rotation_count`]
    /// counts against the on-demand quota.
    pub rotations: Vec<KeyRotation>,
    /// This key's automatic key rotation schedule, or `None` while automatic
    /// rotation of its key material is not enabled — see [`RotationSchedule`].
    /// `EnableKeyRotation` sets it, `DisableKeyRotation` clears it, and
    /// `GetKeyRotationStatus` reports it.
    pub rotation_schedule: Option<RotationSchedule>,
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
            .field("multi_region_topology", &self.multi_region_topology)
            .field("policy", &self.policy)
            .field("deletion_date", &self.deletion_date)
            .field("pending_window_in_days", &self.pending_window_in_days)
            .field("tags", &self.tags)
            // The material itself never appears; its id identifies it without
            // disclosing it.
            .field("material", &"<redacted>")
            .field("current_key_material_id", &self.current_key_material_id)
            // Likewise for the material earlier rotations replaced: only how many
            // versions are retained, never their bytes.
            .field("superseded_material", &self.superseded_material.len())
            .field("rotations", &self.rotations)
            .field("rotation_schedule", &self.rotation_schedule)
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
        let arn = key_arn(&region, &key_id);
        let awaiting_import = matches!(material, KeyMaterial::AwaitingImport);
        // A `MultiRegion` create mints the PRIMARY key of a new set, in the Region
        // it was created in: "creates a multi-Region primary key that you can
        // replicate into other Amazon Web Services Regions"
        // (`smithy_query(query="doc",shape="CreateKeyRequest",member="MultiRegion")`).
        let multi_region_topology =
            multi_region.then(|| MultiRegionTopology::primary_in(region.clone()));
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
            multi_region_topology,
            policy,
            deletion_date: None,
            pending_window_in_days: None,
            tags,
            current_key_material_id: material.id(),
            material,
            superseded_material: Vec::new(),
            rotations: Vec::new(),
            // A new KMS key has no automatic rotation schedule: "when you enable
            // automatic rotation of a customer managed KMS key" is what starts
            // one (`smithy_query(query="doc",shape="EnableKeyRotation")`).
            rotation_schedule: None,
        }
    }

    /// Make `material` this key's current key material, retaining the version it
    /// replaces and recording the rotation that did it.
    ///
    /// "Rotating key material automatically changes the key material for new
    /// encryption operations" while "KMS retains all key material"
    /// (`smithy_query(query="doc",shape="EnableKeyRotation")`), so this moves the
    /// outgoing version to [`Self::superseded_material`] rather than dropping it:
    /// after a rotation the key encrypts under the new material and still opens
    /// ciphertext sealed under any earlier one.
    ///
    /// `rotation_type` is the `RotationType` of the rotation performing it — a
    /// value of `smithy_query(shape="RotationType")`, e.g.
    /// [`ROTATION_TYPE_ON_DEMAND`].
    pub fn rotate_material(&mut self, material: KeyMaterial, rotation_type: &str) {
        let key_material_id = material.id();
        let superseded = std::mem::replace(&mut self.material, material);
        self.superseded_material.push(superseded);
        self.current_key_material_id = key_material_id.clone();
        self.rotations.push(KeyRotation {
            key_material_id,
            rotation_date: SystemTime::now(),
            rotation_type: rotation_type.to_string(),
        });
    }

    /// How many ON_DEMAND rotations this key has had, which is what
    /// [`MAX_ON_DEMAND_ROTATIONS`] bounds. An automatic rotation does not count
    /// against that quota: it is a quota on on-demand rotation alone.
    pub fn on_demand_rotation_count(&self) -> usize {
        self.rotations
            .iter()
            .filter(|rotation| rotation.rotation_type == ROTATION_TYPE_ON_DEMAND)
            .count()
    }

    /// Start or re-configure this key's automatic rotation schedule, from the
    /// rotation period the request named or [`DEFAULT_ROTATION_PERIOD_IN_DAYS`] when
    /// it named none.
    ///
    /// "KMS rotates the key material of the KMS key one year (approximately 365
    /// days) from the enable date and every year thereafter", and
    /// `RotationPeriodInDays` both chooses that period and modifies "the rotation
    /// period of a key that you previously enabled automatic key rotation on"
    /// (`smithy_query(query="doc",shape="EnableKeyRotation")`) — so the schedule is
    /// measured from the call that configured it, and configuring it again measures
    /// the new period from that later call.
    pub fn schedule_rotation(&mut self, period_in_days: Option<i32>) {
        self.rotation_schedule = Some(RotationSchedule {
            period_in_days: period_in_days.unwrap_or(DEFAULT_ROTATION_PERIOD_IN_DAYS),
            scheduled_from: SystemTime::now(),
        });
    }

    /// Stop automatic rotation of this key's key material, leaving the material and
    /// the rotations it has already had exactly as they are: `DisableKeyRotation`
    /// "disables automatic rotation of the key material"
    /// (`smithy_query(query="doc",shape="DisableKeyRotation")`), it does not undo a
    /// rotation.
    pub fn unschedule_rotation(&mut self) {
        self.rotation_schedule = None;
    }

    /// The automatic rotation schedule a READER reports for this key, which is the
    /// one it carries except while it is awaiting deletion: "while a KMS key is
    /// pending deletion, its key rotation status is false and KMS does not rotate
    /// the key material. If you cancel the deletion, the original key rotation
    /// status returns" (`smithy_query(query="doc",shape="GetKeyRotationStatus")`) —
    /// so the schedule is kept on the record and masked at read time, never cleared.
    ///
    /// A disabled key is NOT masked: "the key rotation status does not change when
    /// you disable a KMS key" (same doc).
    pub fn reported_rotation_schedule(&self) -> Option<&RotationSchedule> {
        if self.key_state == KEY_STATE_PENDING_DELETION {
            return None;
        }
        self.rotation_schedule.as_ref()
    }

    /// Whether automatic rotation of this key's key material is supported at all:
    /// "automatic key rotation is supported only on symmetric encryption KMS keys.
    /// You cannot enable automatic rotation of asymmetric KMS keys, HMAC KMS keys,
    /// KMS keys with imported key material, or KMS keys in a custom key store"
    /// (`smithy_query(query="doc",shape="EnableKeyRotation")`). Read from the key's
    /// own kind and the origin of its material rather than from one spec's name.
    pub fn supports_automatic_rotation(&self) -> bool {
        self.is_symmetric_encryption() && self.origin == ORIGIN_AWS_KMS
    }

    /// Whether this key is a *symmetric encryption* KMS key: one whose key material
    /// is an AES-GCM key and whose usage is encryption and decryption. It is the
    /// partition the key-material operations are restricted to — "On-demand key
    /// rotation is supported only on symmetric encryption KMS keys"
    /// (`smithy_query(query="doc",shape="RotateKeyOnDemand")`) — so it is read from
    /// the key's spec profile rather than from one spec's name.
    pub fn is_symmetric_encryption(&self) -> bool {
        self.key_usage == KEY_USAGE_ENCRYPT_DECRYPT
            && key_spec_profile(&self.key_spec)
                .is_some_and(|profile| matches!(profile.material, MaterialKind::Aes256))
    }

    /// Add or edit this key's tags: "To add a tag, specify a new tag key and a tag
    /// value. To edit a tag, specify an existing tag key and a new tag value"
    /// (`smithy_query(query="doc",shape="TagResource")`). A tag key the key already
    /// carries takes the value this request gives it — so the last write of a tag key
    /// is the value the key holds — and every tag the request does not name stays as
    /// it was.
    pub fn apply_tags(&mut self, tags: BTreeMap<String, String>) {
        self.tags.extend(tags);
    }

    /// Remove the tags `tag_keys` names. "If the specified tag key isn't found on the
    /// KMS key, it doesn't throw an exception"
    /// (`smithy_query(query="doc",shape="UntagResource")`), so a key this key does not
    /// carry removes nothing instead of refusing.
    pub fn remove_tags(&mut self, tag_keys: &[String]) {
        for tag_key in tag_keys {
            self.tags.remove(tag_key);
        }
    }

    /// This key's tags as the `TagList` a reader reports, in tag-key order.
    ///
    /// The order is the stored map's, so it is the same on every call and a tag added
    /// later never reshuffles the tags a previous page of `ListResourceTags` returned.
    pub fn tag_list(&self) -> Vec<Tag> {
        self.tags
            .iter()
            .map(|(tag_key, tag_value)| Tag {
                tag_key: tag_key.clone(),
                tag_value: tag_value.clone(),
            })
            .collect()
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
            multi_region: Some(self.is_multi_region()),
            // Both members are projected from the one topology record, so which key
            // of the set this is, which Region holds the primary and which hold the
            // replicas are always reported in agreement.
            multi_region_configuration: self
                .multi_region_topology
                .as_ref()
                .map(|topology| topology.configuration(&self.key_id, &self.region)),
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

    /// Whether this key belongs to a multi-Region key set, which is what the
    /// `MultiRegion` metadata member reports.
    pub fn is_multi_region(&self) -> bool {
        self.multi_region_topology.is_some()
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

/// One completed rotation of a KMS key's key material: which material the rotation
/// made current, when it did, and which kind of rotation it was.
///
/// This is the history `ListKeyRotations` reports ("You can use ListKeyRotations to
/// identify the date that completed on-demand rotations were performed",
/// `smithy_query(query="doc",shape="RotateKeyOnDemand")`), kept oldest first on the
/// key record so the order it is reported in is the order the rotations happened in.
#[derive(Debug, Clone)]
pub struct KeyRotation {
    /// The `KeyMaterialId` of the material this rotation made current, or `None`
    /// for material KMS gives no id — see [`KeyMaterial::id`].
    pub key_material_id: Option<String>,
    /// The instant the rotation completed.
    pub rotation_date: SystemTime,
    /// A `RotationType` value (`smithy_query(shape="RotationType")`).
    pub rotation_type: String,
}

/// One KMS key's automatic key rotation schedule: how long KMS waits between
/// rotations of its key material, and when the current schedule started.
///
/// The next rotation date is DERIVED from the two ([`Self::next_rotation_date`])
/// rather than stored, so it cannot drift from the period a later
/// `EnableKeyRotation` set.
#[derive(Debug, Clone)]
pub struct RotationSchedule {
    /// The number of days between automatic rotations — a
    /// `RotationPeriodInDaysType` (90..2560,
    /// `smithy_query(query="operation",operation="EnableKeyRotation")`).
    pub period_in_days: i32,
    /// The instant the call that configured this schedule was served: the "enable
    /// date" the first rotation is measured from.
    pub scheduled_from: SystemTime,
}

impl RotationSchedule {
    /// When KMS next rotates the key material under this schedule: one rotation
    /// period after the call that configured it, since KMS "rotates the key material
    /// of the KMS key one year (approximately 365 days) from the enable date and
    /// every year thereafter"
    /// (`smithy_query(query="doc",shape="EnableKeyRotation")`), with the period in
    /// place of the year.
    pub fn next_rotation_date(&self) -> SystemTime {
        let period = std::time::Duration::from_secs(
            u64::try_from(self.period_in_days.max(0)).unwrap_or(0) * SECONDS_PER_DAY,
        );
        self.scheduled_from + period
    }
}

/// Seconds in a day, for turning a rotation period in days into a duration.
const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// Why a request to CONFIGURE a KMS key's automatic rotation schedule is refused.
/// One enum for `EnableKeyRotation` and `DisableKeyRotation`, which model the same
/// errors and accept the same keys: "you can enable (EnableKeyRotation) and disable
/// automatic rotation (DisableKeyRotation) of the key material in customer managed
/// KMS keys" (`smithy_query(query="doc",shape="GetKeyRotationStatus")`).
#[derive(Debug, Clone)]
pub enum RotationConfigRefusal {
    /// The key is not one whose automatic rotation is configurable at all —
    /// `UnsupportedOperationException`, "a specified resource is not valid for this
    /// operation". KMS reports a key the rotation operations cannot rotate with the
    /// error code ALONE: the refusal is recorded on the wire with an empty message,
    /// so this variant carries none and [`unsupported_rotation_message`] is what
    /// every rotation operation builds its wire message from.
    Unsupported,
    /// The key is disabled — the modelled `DisabledException`, whose message names
    /// the key and its state ([`invalid_state_message`]).
    Disabled(String),
    /// The key is in some other key state this operation is not compatible with —
    /// `KMSInvalidStateException`, worded the same way.
    InvalidState(String),
}

/// The `message` a rotation operation puts on an `UnsupportedOperationException`:
/// none.
///
/// KMS refuses a key whose kind cannot be rotated with the error code alone — the
/// refusal of an unsupported key type is recorded on the wire with an empty message
/// — so no explanatory text is manufactured for it. The reason the key was refused
/// is stated in a comment at the check that refuses it, not on the wire.
pub fn unsupported_rotation_message() -> Option<String> {
    None
}

/// Whether `key` is one whose automatic key rotation schedule a caller may set or
/// clear, and which refusal reports it when it is not.
///
/// The partition is the documented one: automatic rotation is configurable on a
/// customer managed symmetric encryption KMS key whose key material KMS generated,
/// in a compatible key state, and on the PRIMARY key of a multi-Region key set
/// rather than on a replica — "you cannot enable automatic rotation of asymmetric
/// KMS keys, HMAC KMS keys, KMS keys with imported key material, or KMS keys in a
/// custom key store. To enable or disable automatic rotation of a set of related
/// multi-Region keys, set the property on the primary key. ... You cannot enable or
/// disable automatic rotation of Amazon Web Services managed KMS keys"
/// (`smithy_query(query="doc",shape="EnableKeyRotation")`).
///
/// One function for both operations, so they cannot judge the same key differently.
pub fn check_rotation_configurable(key: &Cmk) -> Result<(), RotationConfigRefusal> {
    // "You cannot enable or disable automatic rotation of Amazon Web Services
    // managed KMS keys. KMS always rotates the key material of Amazon Web Services
    // managed keys every year."
    if key.key_manager != KEY_MANAGER_CUSTOMER {
        return Err(RotationConfigRefusal::Unsupported);
    }

    // Which keys cannot rotate automatically follows from the key's own kind and the
    // origin of its material, never from one spec's name: an asymmetric key and an
    // HMAC key are not symmetric encryption keys, and a key whose material did not
    // come from KMS is a key with imported key material.
    if !key.is_symmetric_encryption() || key.origin != ORIGIN_AWS_KMS {
        return Err(RotationConfigRefusal::Unsupported);
    }

    // "The KMS key that you use for this operation must be in a compatible key
    // state": a disabled key has its own modelled refusal, and any other key state
    // outside `Enabled` is a key state this operation is not compatible with.
    if key.key_state == KEY_STATE_DISABLED {
        return Err(RotationConfigRefusal::Disabled(invalid_state_message(key)));
    }
    if !key.is_usable() {
        return Err(RotationConfigRefusal::InvalidState(invalid_state_message(
            key,
        )));
    }

    // "To enable or disable automatic rotation of a set of related multi-Region
    // keys, set the property on the primary key" — so a replica key of a set does
    // not carry the schedule; the primary key of its set does.
    if key
        .multi_region_topology
        .as_ref()
        .is_some_and(|topology| topology.primary_region != key.region)
    {
        return Err(RotationConfigRefusal::Unsupported);
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Multi-Region key sets
// ---------------------------------------------------------------------------

/// Which Region of one multi-Region key set holds its PRIMARY key, and which
/// Regions hold its replica keys.
///
/// Every key of a multi-Region set shares one key id — "these KMS keys have the
/// same key ID, key material, and other metadata"
/// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`) — and this store holds
/// one record per key id, so the set is ONE record and what distinguishes its
/// members is the Region each of them lives in. Exactly one Region holds the
/// primary key at any instant; every other Region named here holds a replica of
/// it, and which is which is what `MultiRegionConfiguration` reports.
#[derive(Debug, Clone)]
pub struct MultiRegionTopology {
    /// The Region holding the set's primary key: the Region a `MultiRegion` create
    /// minted it in, until `UpdatePrimaryRegion` moves the designation elsewhere.
    pub primary_region: String,
    /// The Regions holding a replica key of the set. Sorted, so a reader reports
    /// the replicas in a stable order across calls.
    pub replica_regions: BTreeSet<String>,
}

impl MultiRegionTopology {
    /// A freshly minted multi-Region primary key's topology: primary in the Region
    /// it was created in, with no replica anywhere until one is created in another
    /// Region.
    pub fn primary_in(region: String) -> Self {
        Self {
            primary_region: region,
            replica_regions: BTreeSet::new(),
        }
    }

    /// Make the replica key in `region` this set's primary key and the former
    /// primary key a replica, answering whether that Region held a replica to
    /// promote at all.
    ///
    /// "This operation changes the replica key in the specified Region to a primary
    /// key and changes the former primary key to a replica key"
    /// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`), so the two moves
    /// are one step: the set never has two primary keys, nor none. A Region holding
    /// no replica of the set — including the one that already holds the primary key
    /// — has nothing to promote and leaves the topology exactly as it was.
    pub fn promote(&mut self, region: &str) -> bool {
        if !self.replica_regions.remove(region) {
            return false;
        }
        let former_primary = std::mem::replace(&mut self.primary_region, region.to_string());
        self.replica_regions.insert(former_primary);
        true
    }

    /// This set as the `MultiRegionConfiguration` of the key `region`'s endpoint
    /// serves: that key is the set's primary exactly when the primary Region is its
    /// own, and `ReplicaKeys` names every Region holding a replica, each by the key
    /// ARN that Region's endpoint serves it under.
    pub fn configuration(&self, key_id: &str, region: &str) -> MultiRegionConfiguration {
        let key_type = if self.primary_region == region {
            MULTI_REGION_KEY_TYPE_PRIMARY
        } else {
            MULTI_REGION_KEY_TYPE_REPLICA
        };
        MultiRegionConfiguration {
            multi_region_key_type: Some(key_type.to_string()),
            primary_key: Some(multi_region_member(key_id, &self.primary_region)),
            replica_keys: Some(
                self.replica_regions
                    .iter()
                    .map(|replica| multi_region_member(key_id, replica))
                    .collect(),
            ),
        }
    }
}

/// One key of a multi-Region set, as `MultiRegionConfiguration` names it: the
/// Region it lives in, and the key ARN that Region's endpoint serves it under.
fn multi_region_member(key_id: &str, region: &str) -> MultiRegionKey {
    MultiRegionKey {
        arn: Some(key_arn(region, key_id)),
        region: Some(region.to_string()),
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

    /// Which of the three key TYPES a key of this spec is: "both symmetric or both
    /// asymmetric or both HMAC"
    /// (`smithy_query(query="doc",shape="UpdateAliasRequest",member="TargetKeyId")`)
    /// is the partition KMS names, and it follows from the spec's material: an
    /// AES-GCM key is a symmetric encryption key, an HMAC key is an HMAC key, and a
    /// key pair is asymmetric.
    ///
    /// The prose is what a refusal names, so it reads as the documentation does.
    pub fn family(&self) -> &'static str {
        match self.material {
            MaterialKind::Aes256 => "symmetric",
            MaterialKind::Hmac(_) => "HMAC",
            MaterialKind::KeyPair => "asymmetric",
        }
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

/// `len` bytes drawn from the OS CSPRNG — the entropy behind a key this service
/// generates for a caller rather than for a KMS key record.
///
/// `None` means the platform refused to produce entropy; the caller answers a
/// modelled server error rather than handing out predictable bytes.
pub fn random_bytes(len: usize) -> Option<Vec<u8>> {
    let mut bytes = vec![0u8; len];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(bytes)
}

/// How many bytes of data key a `DataKeySpec` names, or `None` when it is not a
/// `DataKeySpec` this service knows.
///
/// "Use AES_128 to generate a 128-bit symmetric key, or AES_256 to generate a
/// 256-bit symmetric key"
/// (`smithy_query(query="doc",shape="GenerateDataKeyWithoutPlaintextRequest",member="KeySpec")`),
/// so the length is the bit count the spec names divided by eight — read from the
/// spec itself, which is why every value of
/// [`DATA_KEY_SPEC_VALUES`](crate::ops::types::DATA_KEY_SPEC_VALUES) maps without a
/// per-value case.
pub fn data_key_bytes(key_spec: &str) -> Option<usize> {
    if !DATA_KEY_SPEC_VALUES.contains(&key_spec) {
        return None;
    }
    let bits: usize = key_spec.strip_prefix("AES_")?.parse().ok()?;
    (bits > 0 && bits.is_multiple_of(8)).then_some(bits / 8)
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

/// The tag-key prefix AWS reserves for its own tags: "Do not use the `aws:` prefix
/// in your tag names or values because it is reserved for Amazon Web Services use.
/// You can't edit or delete tag names or values with this prefix" (Tagging Amazon
/// Web Services resources in the General Reference, which `TagResource`'s own
/// documentation points at for "the format and syntax" of a tag). A caller's tag
/// cannot fall under it, so a request that carries one is refused and nothing is
/// stored.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `tag_key` falls under [`RESERVED_TAG_KEY_PREFIX`].
///
/// The prefix is matched without regard to case: the reservation is on the prefix,
/// not on one spelling of it, so no capitalisation of it is a caller's to set.
pub fn is_reserved_tag_key(tag_key: &str) -> bool {
    let prefix = RESERVED_TAG_KEY_PREFIX.as_bytes();
    tag_key
        .as_bytes()
        .get(..prefix.len())
        .is_some_and(|head| head.eq_ignore_ascii_case(prefix))
}

/// The tags a request's `TagList` asks a key to carry, keyed by tag key, or why the
/// list is not one KMS stores — the sentence a `TagException` ("one or more tags are
/// not valid") carries.
///
/// ONE function for every operation that accepts tags, so no two of them judge the
/// same list differently:
///
/// * "You cannot have more than one tag on a KMS key with the same tag key"
///   (`smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`), so a
///   list naming one tag key twice is refused rather than silently collapsed to
///   whichever entry came last;
/// * a tag key under [`RESERVED_TAG_KEY_PREFIX`] is not a caller's to set.
///
/// Every other property of a tag — that both members are present, and their lengths
/// — is modelled, so the boundary validator has already refused a list that breaks
/// one before any handler runs. In particular a tag VALUE may be the empty string
/// (`TagValueType` is `@length(0..256)`), which is why nothing here treats an empty
/// value as absent.
///
/// The sentences are this emulator's wording; what this function pins is WHICH tag
/// lists are refused.
pub fn requested_tags(tags: &[Tag]) -> Result<BTreeMap<String, String>, String> {
    let mut requested = BTreeMap::new();
    for tag in tags {
        if is_reserved_tag_key(&tag.tag_key) {
            // KMS words this refusal by naming the reserved prefix and nothing else,
            // so the sentence carries no value from the request.
            return Err(format!(
                "Tags beginning with {RESERVED_TAG_KEY_PREFIX} are reserved"
            ));
        }
        if requested
            .insert(tag.tag_key.clone(), tag.tag_value.clone())
            .is_some()
        {
            return Err(format!(
                "One or more tags are not valid: the tag key {} appears more than once.",
                tag.tag_key
            ));
        }
    }
    Ok(requested)
}

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

/// The only key policy name KMS serves: "If no policy name is specified, the
/// default value is `default`. The only valid name is `default`"
/// (`smithy_query(query="doc",shape="GetKeyPolicyRequest",member="PolicyName")`).
/// A request that omits `PolicyName` names this one, and a request naming anything
/// else names a policy no key has.
pub const KEY_POLICY_NAME_DEFAULT: &str = "default";

/// The largest key policy document a key holds: KMS's published resource quota
/// "Key policy document size: 32 KB" (Quotas in the Key Management Service
/// Developer Guide, which the modelled `LimitExceededException` — "a length
/// constraint or quota was exceeded" — points at). It is what "if the key policy
/// exceeds the length constraint, KMS returns a `LimitExceededException`"
/// (`smithy_query(query="doc",shape="PutKeyPolicyRequest",member="Policy")`) is
/// measured against here: the modelled `@length(1..131072)` is refused by the
/// boundary validator before any handler runs, so the quota is the bound a handler
/// still gets to enforce — and it is also what bounds how far one request can grow
/// this store.
pub const MAX_KEY_POLICY_BYTES: usize = 32 * 1024;

/// Whether `policy` is a key policy document KMS would accept, and why not when it
/// is not — the sentence a `MalformedPolicyDocumentException` ("the specified policy
/// is not syntactically or semantically correct") carries.
///
/// The criteria are the `Policy` member's own
/// (`smithy_query(query="doc",shape="PutKeyPolicyRequest",member="Policy")`, which
/// `CreateKeyRequest` and `ReplicateKeyRequest` repeat verbatim), so ONE function
/// judges a key policy wherever a request carries one:
///
/// * it is an IAM JSON policy document — a JSON object carrying a `Statement`,
///   either one statement object or a non-empty list of them, each with an `Effect`
///   of `Allow` or `Deny`;
/// * "each statement in the key policy must contain one or more principals", which
///   is the criterion that separates a KEY policy from an identity policy.
///
/// What it deliberately does NOT refuse is a statement missing `Action` or
/// `Resource`: "if either of the required `Resource` or `Action` elements are
/// missing from a key policy statement, the policy statement has no effect … but the
/// `PutKeyPolicy` API request succeeds, even though the policy statement is
/// ineffective" (same doc). Nor does it judge the policy-lockout safety check: that
/// check asks whether the policy lets the CALLING PRINCIPAL make a subsequent
/// `PutKeyPolicy` request, which needs the caller's IAM identity — something this
/// emulator does not model — so `BypassPolicyLockoutSafetyCheck` is accepted and has
/// no effect, exactly as `CreateKey` already records.
///
/// The sentences are this emulator's wording; what this function pins is WHICH
/// documents are refused.
pub fn validate_key_policy(policy: &str) -> Result<(), String> {
    let document: ::serde_json::Value = ::serde_json::from_str(policy)
        .map_err(|error| format!("The key policy is not valid JSON: {error}."))?;
    let Some(object) = document.as_object() else {
        return Err("The key policy must be a JSON object.".to_string());
    };
    // An IAM JSON policy document's `Statement` is either one statement or a list
    // of them; a key policy with no statement at all grants nothing and is not a
    // document KMS stores.
    let statements: Vec<&::serde_json::Value> = match object.get("Statement") {
        Some(::serde_json::Value::Array(statements)) if !statements.is_empty() => {
            statements.iter().collect()
        }
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement],
        Some(_) => {
            return Err(
                "The key policy's Statement must be a statement or a non-empty list of \
                 statements."
                    .to_string(),
            )
        }
        None => return Err("The key policy must contain a Statement.".to_string()),
    };
    for (index, statement) in statements.iter().enumerate() {
        let Some(statement) = statement.as_object() else {
            return Err(format!(
                "Statement {index} of the key policy must be a JSON object."
            ));
        };
        match statement.get("Effect").and_then(::serde_json::Value::as_str) {
            Some("Allow" | "Deny") => {}
            _ => {
                return Err(format!(
                    "Statement {index} of the key policy must have an Effect of Allow or Deny."
                ))
            }
        }
        if !names_a_principal(statement) {
            return Err(format!(
                "Statement {index} of the key policy must contain one or more principals."
            ));
        }
    }
    Ok(())
}

/// Whether one policy statement names at least one principal, in either of the two
/// elements that name principals (`Principal`, and its negation `NotPrincipal`).
fn names_a_principal(statement: &::serde_json::Map<String, ::serde_json::Value>) -> bool {
    ["Principal", "NotPrincipal"]
        .iter()
        .any(|element| statement.get(*element).is_some_and(is_non_empty_principal))
}

/// Whether a `Principal` element names anybody: the element is a string (`"*"`), a
/// map from principal kind to one principal or a list of them, or a list — so it
/// names somebody exactly when some leaf is a non-empty string.
fn is_non_empty_principal(value: &::serde_json::Value) -> bool {
    match value {
        ::serde_json::Value::String(principal) => !principal.is_empty(),
        ::serde_json::Value::Array(principals) => principals.iter().any(is_non_empty_principal),
        ::serde_json::Value::Object(by_kind) => by_kind.values().any(is_non_empty_principal),
        _ => false,
    }
}

/// How a request naming a key policy other than the one KMS serves is refused.
///
/// `GetKeyPolicy` and `PutKeyPolicy` both take a `PolicyName`, and the only name a
/// key has a policy under is [`KEY_POLICY_NAME_DEFAULT`], so any other name asks for
/// a policy that does not exist on the key — a `NotFoundException` ("the specified
/// entity or resource could not be found"), naming the key by its ARN as every other
/// refusal in this service does. One function for both operations, so a reader and a
/// writer cannot judge the same name differently.
pub fn policy_not_found_message(key: &Cmk, policy_name: &str) -> String {
    format!(
        "No such policy exists: {policy_name}. The only valid key policy name for {} is \
         \"{KEY_POLICY_NAME_DEFAULT}\".",
        key.arn
    )
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

/// `material` as an AES-256-GCM key, or `None` when it is not AES material and so
/// has nothing to seal or open with.
fn cipher(material: &KeyMaterial) -> Option<LessSafeKey> {
    UnboundKey::new(&AES_256_GCM, material.aes256()?)
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
    let cipher = cipher(&key.material)?;
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

/// What opening a ciphertext envelope yielded: the plaintext, and which version of
/// the key's material opened it.
pub struct Opened {
    /// The recovered plaintext.
    pub plaintext: Vec<u8>,
    /// The `KeyMaterialId` of the material that opened the ciphertext — the
    /// material that sealed it, which after a rotation need not be the key's
    /// current one.
    pub key_material_id: Option<String>,
}

/// Open an envelope with `key`'s material under `context`.
///
/// Every version of the key's material is tried: the one current now, then the ones
/// rotation replaced, newest first. A KMS key keeps the material it has encrypted
/// under, so a ciphertext sealed before a rotation still decrypts after it, and the
/// authentication tag is what identifies the version that sealed it.
///
/// `None` means no version verified the tag: the ciphertext was sealed by another
/// key, under a different encryption context, or has been altered.
pub fn unseal(
    key: &Cmk,
    envelope: &Envelope,
    context: Option<&HashMap<String, String>>,
) -> Option<Opened> {
    let aad = encryption_context_aad(context);
    std::iter::once(&key.material)
        .chain(key.superseded_material.iter().rev())
        .find_map(|material| {
            let cipher = cipher(material)?;
            let mut body = envelope.body.clone();
            let plaintext = cipher
                .open_in_place(
                    Nonce::assume_unique_for_key(envelope.nonce),
                    Aad::from(aad.as_slice()),
                    &mut body,
                )
                .ok()?;
            Some(Opened {
                plaintext: plaintext.to_vec(),
                key_material_id: material.id(),
            })
        })
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
// Grants
// ---------------------------------------------------------------------------

/// The largest number of grants this emulator's store holds for ONE KMS key.
///
/// A bound the EMULATOR imposes, not the quota KMS publishes: `CreateGrant` is
/// reachable by any caller and a grant without a `Name` is a new record every time,
/// so without a cap a loop of valid creates grows the process without end. Past it
/// `CreateGrant` refuses with the modelled `LimitExceededException` — "a length
/// constraint or quota was exceeded" — and persists nothing.
pub const MAX_GRANTS_PER_KEY: usize = 500;

/// One grant on a KMS key: "a policy instrument that allows Amazon Web Services
/// principals to use KMS keys in cryptographic operations"
/// (`smithy_query(query="doc",shape="CreateGrant")`).
#[derive(Debug, Clone)]
pub struct Grant {
    /// The minted grant id, which is also this record's map key.
    pub grant_id: String,
    /// The key id of the KMS key the grant applies to. A grant records the key it
    /// RESOLVED to, never the alias the request named, so repointing an alias
    /// cannot move an existing grant to another key.
    pub key_id: String,
    /// That key's ARN, which is what a grant reader reports as the entry's `KeyId`:
    /// `KeyIdType` carries either form, and KMS reports the grant's key as the ARN.
    pub key_arn: String,
    /// The Region whose endpoint the grant was created through. A KMS key is a
    /// regional resource, so its grants are too.
    pub region: String,
    /// "The Amazon Web Services account under which the grant was issued"
    /// (`smithy_query(shape="GrantListEntry")` -> `IssuingAccount`).
    pub issuing_account: String,
    /// The `Name` the request gave, if any: "If a name was provided in the
    /// CreateGrant request, that name is returned. Otherwise this value is null"
    /// (`smithy_query(shape="GrantListEntry")` -> `Name`).
    pub name: Option<String>,
    pub creation_date: SystemTime,
    pub grantee_principal: Option<String>,
    pub grantee_service_principal: Option<String>,
    pub retiring_principal: Option<String>,
    pub retiring_service_principal: Option<String>,
    /// "The list of operations permitted by the grant", in the order the request
    /// listed them.
    pub operations: Vec<String>,
    pub constraints: Option<GrantConstraints>,
}

impl Grant {
    /// This grant's `GrantListEntry`, computed from the record — the one place a
    /// grant is projected onto the wire, so two readers cannot disagree.
    pub fn list_entry(&self) -> GrantListEntry {
        GrantListEntry {
            key_id: Some(self.key_arn.clone()),
            grant_id: Some(self.grant_id.clone()),
            name: self.name.clone(),
            creation_date: Some(date_time(self.creation_date)),
            grantee_principal: self.grantee_principal.clone(),
            retiring_principal: self.retiring_principal.clone(),
            issuing_account: Some(self.issuing_account.clone()),
            operations: Some(self.operations.clone()),
            constraints: self.constraints.clone(),
            grantee_service_principal: self.grantee_service_principal.clone(),
            retiring_service_principal: self.retiring_service_principal.clone(),
        }
    }

    /// Everything about this grant that came from the request that created it,
    /// digested into 64 hex characters.
    ///
    /// "When this value is present, you can retry a `CreateGrant` request with
    /// identical parameters; if the grant already exists, the original `GrantId` is
    /// returned without creating a new grant"
    /// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Name")`), so a
    /// retry has to be recognisable from the request alone — this is what "identical
    /// parameters" is compared as.
    ///
    /// The permitted operations are sorted and de-duplicated first: they are a SET
    /// of permissions, so a request listing the same ones in another order is the
    /// same request. The encryption-context constraints are maps, and are
    /// canonicalized the same way for the same reason. Every part is
    /// length-prefixed, so no two different requests can digest to the same bytes.
    /// The grant id, the creation date and the grant tokens are NOT part of it: the
    /// first two are what a retry is allowed to reuse, and the last is "unique with
    /// every `CreateGrant` request".
    pub fn fingerprint(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(b"kms-grant\0");
        for part in [
            self.key_id.as_str(),
            self.name.as_deref().unwrap_or_default(),
            self.grantee_principal.as_deref().unwrap_or_default(),
            self.grantee_service_principal.as_deref().unwrap_or_default(),
            self.retiring_principal.as_deref().unwrap_or_default(),
            self.retiring_service_principal.as_deref().unwrap_or_default(),
        ] {
            feed(&mut hasher, part);
        }

        let mut operations: Vec<&str> = self.operations.iter().map(String::as_str).collect();
        operations.sort_unstable();
        operations.dedup();
        for operation in operations {
            feed(&mut hasher, operation);
        }

        let constraints = self.constraints.as_ref();
        feed(
            &mut hasher,
            constraints
                .and_then(|constraints| constraints.source_arn.as_deref())
                .unwrap_or_default(),
        );
        feed_context(
            &mut hasher,
            "EncryptionContextEquals",
            constraints.and_then(|constraints| constraints.encryption_context_equals.as_ref()),
        );
        feed_context(
            &mut hasher,
            "EncryptionContextSubset",
            constraints.and_then(|constraints| constraints.encryption_context_subset.as_ref()),
        );

        hex::encode(hasher.finalize())
    }

    /// Whether this grant permits `operation` on the key `key_id` under the request's
    /// encryption context `context`.
    ///
    /// A grant covers ONE key, so a grant on another key permits nothing here: "a
    /// grant on one KMS key does not authorize use of another". It covers exactly the
    /// operations it lists. And its encryption-context constraints "allow the
    /// permissions in the grant only when the encryption context in the request
    /// matches (EncryptionContextEquals) or includes (EncryptionContextSubset) the
    /// encryption context specified in the constraint"
    /// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Constraints")`),
    /// and they "apply only to cryptographic operations that support an encryption
    /// context" — so an operation that takes none is judged without them.
    ///
    /// A `SourceArn` constraint restricts "an Amazon Web Services resource on behalf
    /// of which the request is made"; nothing in a direct request names such a
    /// resource, so it narrows nothing here.
    pub fn permits(
        &self,
        key_id: &str,
        operation: &str,
        context: Option<&HashMap<String, String>>,
    ) -> bool {
        if self.key_id != key_id {
            return false;
        }
        if !self
            .operations
            .iter()
            .any(|permitted| permitted == operation)
        {
            return false;
        }
        let takes_context = GRANT_OPERATION_PROFILES
            .iter()
            .find(|profile| profile.operation == operation)
            .is_some_and(|profile| profile.encryption_context);
        if !takes_context {
            return true;
        }
        self.constraints
            .as_ref()
            .is_none_or(|constraints| encryption_context_allows(constraints, context))
    }
}

/// Why a request's grant tokens do not authorize it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantRefusal {
    /// A token is not one this service minted, so it names no grant at all — which is
    /// what makes the GRANT TOKEN the thing that "is not valid".
    MalformedToken,
    /// The tokens are well formed, but no grant they name authorizes the request: the
    /// grant is gone, is on another key, does not permit the operation, or its
    /// encryption-context constraint is not met. Permission that would have come from
    /// a grant is absent, which is an authorization failure rather than a bad request.
    NotAuthorized,
}

/// Whether a grant's encryption-context constraints allow a request carrying
/// `context`.
///
/// "In a grant constraint, the key in each key-value pair is not case sensitive, but
/// the value is case sensitive"
/// (`smithy_query(query="doc",shape="GrantConstraints")`), so the keys are folded and
/// the values are not. An absent context is an empty one: `EncryptionContextEquals`
/// then allows only an empty constraint, and `EncryptionContextSubset` only one with
/// no pairs to include.
fn encryption_context_allows(
    constraints: &GrantConstraints,
    context: Option<&HashMap<String, String>>,
) -> bool {
    let empty = HashMap::new();
    let request = folded_keys(context.unwrap_or(&empty));

    if let Some(equals) = constraints.encryption_context_equals.as_ref() {
        if folded_keys(equals) != request {
            return false;
        }
    }
    if let Some(subset) = constraints.encryption_context_subset.as_ref() {
        if !folded_keys(subset)
            .into_iter()
            .all(|(key, value)| request.get(&key) == Some(&value))
        {
            return false;
        }
    }
    true
}

/// An encryption context with its keys folded to lower case, so two contexts that
/// differ only in the case of their KEYS compare equal while their values stay
/// case-sensitive.
fn folded_keys(context: &HashMap<String, String>) -> BTreeMap<String, String> {
    context
        .iter()
        .map(|(key, value)| (key.to_lowercase(), value.clone()))
        .collect()
}

/// Add one length-prefixed part to a digest, so two different sequences of parts
/// cannot run together into the same bytes.
fn feed(hasher: &mut Sha256, part: &str) {
    let len = u32::try_from(part.len()).unwrap_or(u32::MAX);
    hasher.update(len.to_be_bytes());
    hasher.update(part.as_bytes());
}

/// [`feed`] for an encryption-context map, under `label` so the two constraint maps
/// cannot be swapped without changing the digest. The entries are sorted first: the
/// map is unordered, so only its content may decide the digest.
fn feed_context(hasher: &mut Sha256, label: &str, context: Option<&HashMap<String, String>>) {
    feed(hasher, label);
    let mut entries: Vec<(&str, &str)> = context
        .map(|context| {
            context
                .iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    entries.sort_unstable();
    for (key, value) in entries {
        feed(hasher, key);
        feed(hasher, value);
    }
}

/// The account ARN KMS reports as a grant's `IssuingAccount`: the emulator's own
/// account, which is the account that owns every key here.
pub fn issuing_account_arn() -> String {
    ::engine::emulator_arn("iam", "", "root")
}

/// How KMS words `DryRunOperationException`, "the request was rejected because the
/// DryRun parameter was specified"
/// (`smithy_query(query="doc",shape="DryRunOperationException")`).
///
/// One constant for the whole service, so no two operations word the same refusal
/// differently.
pub const DRY_RUN_MESSAGE: &str =
    "The request would have succeeded, but the DryRun option is set.";

/// How a `GrantId` that cannot name any grant is refused: `InvalidGrantIdException`'s
/// message names the value the request gave, since that string is the whole of what
/// is wrong with it.
///
/// One function for the whole service, so no two operations word the same malformed
/// grant id differently.
pub fn invalid_grant_id_message(requested: &str) -> String {
    format!("The specified GrantId is not valid: {requested}.")
}

/// How a grant that is not held is refused: the grant id the request gave, since a
/// grant that was never created, one already retired and one already revoked are the
/// same absence.
///
/// One function for the whole service, so no two operations word it differently.
pub fn grant_not_found_message(grant_id: &str) -> String {
    format!("Grant ID {grant_id} not found")
}

/// How a `RetireGrant` is refused when the grant it identified is not held.
///
/// "Permission to retire a grant is determined primarily by the grant"
/// (`smithy_query(query="doc",shape="RetireGrant")`), so with no such grant there is
/// no permission to retire one, and the refusal names the grant the request gave.
pub fn retire_grant_access_denied_message(grant_id: &str) -> String {
    format!("No grant with the id {grant_id} authorizes kms:RetireGrant.")
}

/// How a request whose only permission would have come from a grant is refused when
/// no grant it named gives that permission.
///
/// The message is built from the request — the operation asked for and the key it
/// names — rather than from a caller identity, which this emulator does not have.
///
/// One function for the whole service, so no two operations word it differently.
pub fn grant_access_denied_message(operation: &str, key_arn: &str) -> String {
    format!("The grant tokens in the request do not authorize kms:{operation} on {key_arn}.")
}

/// A freshly minted grant id: the 64 lowercase hex characters KMS gives a grant,
/// drawn from the OS CSPRNG so two grants never share one.
///
/// `None` means the platform refused to produce entropy; the caller answers a
/// modelled server error rather than minting a predictable id.
pub fn mint_grant_id() -> Option<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(hex::encode(bytes))
}

/// How many characters a grant id has: [`mint_grant_id`] draws 32 bytes, and hex
/// doubles them.
const GRANT_ID_HEX_LEN: usize = 64;

/// Whether `grant_id` has the form a grant id takes.
///
/// `GrantIdType`'s modelled 1..128 length accepts far more than a grant id can be,
/// so the form is what identifies a value that cannot name any grant — as opposed to
/// a well-formed id that simply names none.
pub fn names_a_grant_id(grant_id: &str) -> bool {
    grant_id.len() == GRANT_ID_HEX_LEN
        && grant_id.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// Tags a grant token as one this service minted, and distinguishes it from the
/// pagination tokens, which share the encoding.
const GRANT_TOKEN_PREFIX: &str = "grant-token/1/";

/// A freshly minted grant token for `grant_id`.
///
/// "The returned grant token is unique with every `CreateGrant` request, even when a
/// duplicate `GrantId` is returned. All grant tokens for the same grant ID can be
/// used interchangeably"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Name")`): the token
/// therefore carries the grant id it belongs to plus a fresh nonce, so a new one is
/// minted per call and any of them names the same grant. It is self-describing
/// rather than persisted, so a grant does not accumulate a token per retry.
///
/// `None` means the platform refused to produce entropy.
pub fn mint_grant_token(grant_id: &str) -> Option<String> {
    let mut nonce = [0u8; 16];
    SystemRandom::new().fill(&mut nonce).ok()?;
    Some(hex::encode(format!(
        "{GRANT_TOKEN_PREFIX}{grant_id}/{}",
        hex::encode(nonce)
    )))
}

/// The grant id a grant token names, or `None` when the token is not one this
/// service minted — which is what makes a foreign token an
/// `InvalidGrantTokenException` instead of being silently ignored.
pub fn decode_grant_token(token: &str) -> Option<String> {
    let decoded = hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let rest = text.strip_prefix(GRANT_TOKEN_PREFIX)?;
    let (grant_id, nonce) = rest.split_once('/')?;
    (!grant_id.is_empty() && !nonce.is_empty()).then(|| grant_id.to_string())
}

/// Whether `principal` is given the way KMS requires a grant principal to be: "use
/// the Amazon Resource Name (ARN) of an Amazon Web Services principal"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="GranteePrincipal")`).
///
/// `PrincipalIdType`'s modelled pattern accepts any word-ish string, so the ARN form
/// is what is left to check: `arn:<partition>:<service>:<region>:<account>:<resource>`
/// with a non-empty partition, service, account and resource. The Region segment may
/// be empty, because an IAM principal ARN carries none.
pub fn names_a_principal_arn(principal: &str) -> bool {
    let Some(rest) = principal.strip_prefix("arn:") else {
        return false;
    };
    let segments: Vec<&str> = rest.splitn(5, ':').collect();
    let [partition, service, _region, account, resource] = segments.as_slice() else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !account.is_empty() && !resource.is_empty()
}

/// What one `GrantOperation` needs of the KMS key it is granted on.
///
/// "This list must include only operations that are permitted in a grant. Also, the
/// operation must be supported on the KMS key. For example, you cannot create a
/// grant for a symmetric encryption KMS key that allows the `Sign` operation, or a
/// grant for an asymmetric KMS key that allows the `GenerateDataKey` operation"
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="Operations")`) —
/// the first example is a `KeyUsage` mismatch and the second a key-TYPE mismatch, so
/// a profile records both.
#[derive(Debug, Clone, Copy)]
pub struct GrantOperationProfile {
    /// The `GrantOperation` value this profile describes
    /// (`crate::ops::types::GRANT_OPERATION_VALUES`).
    pub operation: &'static str,
    /// The `KeyUsage` values a key must carry for this operation. EMPTY means the
    /// operation is key management, which every usage permits.
    pub usages: &'static [&'static str],
    /// The key types ([`KeySpecProfile::family`]) that support this operation. EMPTY
    /// means every type does.
    pub families: &'static [&'static str],
    /// Whether the operation this grants takes an `EncryptionContext`. The
    /// encryption-context grant constraints "apply only to cryptographic operations
    /// that support an encryption context … not … operations that do not support an
    /// encryption context, such as cryptographic operations with asymmetric KMS keys
    /// and management operations"
    /// (`smithy_query(query="doc",shape="GrantConstraints")`), so this is what decides
    /// whether a constraint is applied at all.
    pub encryption_context: bool,
}

/// What each `GrantOperation` needs of the key, from the `KeyUsage` matrix
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="KeyUsage")`) and the
/// operation each one grants: a cryptographic operation needs the usage that
/// operation runs under, and the key type whose material can perform it, while
/// `CreateGrant`, `DescribeKey` and `RetireGrant` are management operations that
/// every key supports.
pub const GRANT_OPERATION_PROFILES: &[GrantOperationProfile] = &[
    GrantOperationProfile {
        operation: "CreateGrant",
        usages: &[],
        families: &[],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "DescribeKey",
        usages: &[],
        families: &[],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "RetireGrant",
        usages: &[],
        families: &[],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "Encrypt",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "Decrypt",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "ReEncryptFrom",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "ReEncryptTo",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[],
        encryption_context: true,
    },
    // A data key is generated under a SYMMETRIC encryption key, which is what makes
    // `GenerateDataKey` on an asymmetric key the documented example of an operation
    // the key does not support.
    GrantOperationProfile {
        operation: "GenerateDataKey",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[FAMILY_SYMMETRIC],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "GenerateDataKeyWithoutPlaintext",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[FAMILY_SYMMETRIC],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "GenerateDataKeyPair",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[FAMILY_SYMMETRIC],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "GenerateDataKeyPairWithoutPlaintext",
        usages: &[KEY_USAGE_ENCRYPT_DECRYPT],
        families: &[FAMILY_SYMMETRIC],
        encryption_context: true,
    },
    GrantOperationProfile {
        operation: "Sign",
        usages: &[KEY_USAGE_SIGN_VERIFY],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "Verify",
        usages: &[KEY_USAGE_SIGN_VERIFY],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "GenerateMac",
        usages: &[KEY_USAGE_GENERATE_VERIFY_MAC],
        families: &[FAMILY_HMAC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "VerifyMac",
        usages: &[KEY_USAGE_GENERATE_VERIFY_MAC],
        families: &[FAMILY_HMAC],
        encryption_context: false,
    },
    GrantOperationProfile {
        operation: "DeriveSharedSecret",
        usages: &[KEY_USAGE_KEY_AGREEMENT],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
    // The public half exists only for a key pair, under any of the three usages an
    // asymmetric spec can carry.
    GrantOperationProfile {
        operation: "GetPublicKey",
        usages: &[
            KEY_USAGE_ENCRYPT_DECRYPT,
            KEY_USAGE_SIGN_VERIFY,
            KEY_USAGE_KEY_AGREEMENT,
        ],
        families: &[FAMILY_ASYMMETRIC],
        encryption_context: false,
    },
];

/// The key type [`KeySpecProfile::family`] reports for a symmetric encryption key.
pub const FAMILY_SYMMETRIC: &str = "symmetric";
/// See [`FAMILY_SYMMETRIC`].
pub const FAMILY_ASYMMETRIC: &str = "asymmetric";
/// See [`FAMILY_SYMMETRIC`].
pub const FAMILY_HMAC: &str = "HMAC";

/// Why `key` does not support the grant operation `operation`, or `None` when it
/// does.
///
/// An operation [`GRANT_OPERATION_PROFILES`] does not describe is reported as
/// supported: the modelled `GrantOperation` enum refuses an unknown value before any
/// handler runs, so reaching here with one is not possible, and a value added to the
/// model later must not be refused by a table that has not caught up.
pub fn grant_operation_refusal(key: &Cmk, operation: &str) -> Option<String> {
    let profile = GRANT_OPERATION_PROFILES
        .iter()
        .find(|profile| profile.operation == operation)?;

    if !profile.usages.is_empty() && !profile.usages.contains(&key.key_usage.as_str()) {
        return Some(format!(
            "Operation {operation} is not permitted in a grant on {}, whose KeyUsage is \
             {}; it requires a KeyUsage of {}.",
            key.arn,
            key.key_usage,
            profile.usages.join(" or ")
        ));
    }

    let family = key_spec_profile(&key.key_spec).map(KeySpecProfile::family);
    if !profile.families.is_empty()
        && family.is_none_or(|family| !profile.families.contains(&family))
    {
        return Some(format!(
            "Operation {operation} is not permitted in a grant on {}, whose KeySpec {} is \
             {}; it requires {} key material.",
            key.arn,
            key.key_spec,
            family.unwrap_or("of an unknown type"),
            profile.families.join(" or ")
        ));
    }

    None
}

// ---------------------------------------------------------------------------
// List pagination
// ---------------------------------------------------------------------------

/// Tags a `NextMarker` as one this service minted, and for which list.
const MARKER_PREFIX: &str = "keys/1/";

/// [`MARKER_PREFIX`], for the alias list. A token is tagged per LIST so a marker
/// from one list is not accepted by another, which would silently resume a page at
/// an item of the wrong kind instead of answering `InvalidMarkerException`.
const ALIAS_MARKER_PREFIX: &str = "aliases/1/";

/// [`MARKER_PREFIX`], for one key's tag list.
const RESOURCE_TAG_MARKER_PREFIX: &str = "resource-tags/1/";

/// [`MARKER_PREFIX`], for the grant list.
const GRANT_MARKER_PREFIX: &str = "grants/1/";

/// [`MARKER_PREFIX`], for the retirable-grant list, which pages over a different
/// selection of grants than the per-key list and so mints its own tokens.
const RETIRABLE_GRANT_MARKER_PREFIX: &str = "retirable-grants/1/";

/// The `NextMarker` that resumes a list after `key_id`.
///
/// Hex-encoded so the token is opaque to a caller (who must pass it back
/// verbatim) while still satisfying `MarkerType`'s `^[\u0020-\u00FF]*$` pattern.
pub fn encode_marker(key_id: &str) -> String {
    encode_marker_for(MARKER_PREFIX, key_id)
}

/// The key id a `Marker` resumes after, or `None` when the token is not one this
/// service minted.
pub fn decode_marker(marker: &str) -> Option<String> {
    decode_marker_for(MARKER_PREFIX, marker)
}

/// [`encode_marker`], for the alias list: the token resumes after the alias whose
/// ARN is `alias_arn`, since the ARN is what identifies an alias.
pub fn encode_alias_marker(alias_arn: &str) -> String {
    encode_marker_for(ALIAS_MARKER_PREFIX, alias_arn)
}

/// [`decode_marker`], for the alias list.
pub fn decode_alias_marker(marker: &str) -> Option<String> {
    decode_marker_for(ALIAS_MARKER_PREFIX, marker)
}

/// [`encode_marker`], for one key's tag list: the token resumes after the tag whose
/// key is `tag_key`, since the tag key is what identifies a tag on a key.
pub fn encode_resource_tag_marker(tag_key: &str) -> String {
    encode_marker_for(RESOURCE_TAG_MARKER_PREFIX, tag_key)
}

/// [`decode_marker`], for one key's tag list.
pub fn decode_resource_tag_marker(marker: &str) -> Option<String> {
    decode_marker_for(RESOURCE_TAG_MARKER_PREFIX, marker)
}

/// [`encode_marker`], for the grant list: the token resumes after the grant whose
/// id is `grant_id`.
pub fn encode_grant_marker(grant_id: &str) -> String {
    encode_marker_for(GRANT_MARKER_PREFIX, grant_id)
}

/// [`decode_marker`], for the grant list.
pub fn decode_grant_marker(marker: &str) -> Option<String> {
    decode_marker_for(GRANT_MARKER_PREFIX, marker)
}

/// [`encode_marker`], for the retirable-grant list.
pub fn encode_retirable_grant_marker(grant_id: &str) -> String {
    encode_marker_for(RETIRABLE_GRANT_MARKER_PREFIX, grant_id)
}

/// [`decode_marker`], for the retirable-grant list.
pub fn decode_retirable_grant_marker(marker: &str) -> Option<String> {
    decode_marker_for(RETIRABLE_GRANT_MARKER_PREFIX, marker)
}

/// One encoding for every list, so no two pagers mint tokens a reader must handle
/// differently.
fn encode_marker_for(prefix: &str, id: &str) -> String {
    hex::encode(format!("{prefix}{id}"))
}

/// The inverse of [`encode_marker_for`], for the list `prefix` tags.
fn decode_marker_for(prefix: &str, marker: &str) -> Option<String> {
    let decoded = hex::decode(marker).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    let id = text.strip_prefix(prefix)?;
    (!id.is_empty()).then(|| id.to_string())
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
