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
use std::time::{Duration, SystemTime};

use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use sha2::{Digest, Sha256};

use crate::ops::types::{
    AliasListEntry, CustomKeyStoresListEntry, GrantConstraints, GrantListEntry, KeyLastUsageData,
    KeyMetadata, MultiRegionConfiguration, MultiRegionKey, Tag, XksProxyConfigurationType,
    CUSTOMER_MASTER_KEY_SPEC_VALUES, DATA_KEY_SPEC_VALUES,
};

/// Who a request speaks for, and whether the key policy, the caller's identity policies
/// and the grants in force let it do what it asks — the one authorization decision this
/// service's operations consult.
pub mod authorization;

/// The CloudHSM cluster behind an `AWS_CLOUDHSM` custom key store, resolved against the
/// CloudHSM service's own records — the one place this service judges a cluster.
pub mod cloud_hsm;

/// The `ExpirationModelType` of imported key material that expires, which is the
/// default: "the default is KEY_MATERIAL_EXPIRES"
/// (`smithy_query(query="doc",shape="ImportKeyMaterialRequest",member="ExpirationModel")`).
pub const EXPIRATION_MODEL_EXPIRES: &str = "KEY_MATERIAL_EXPIRES";

/// The `ExpirationModelType` of imported key material that does not expire. See
/// [`EXPIRATION_MODEL_EXPIRES`].
pub const EXPIRATION_MODEL_DOES_NOT_EXPIRE: &str = "KEY_MATERIAL_DOES_NOT_EXPIRE";

/// The furthest ahead imported key material may be set to expire: "the maximum value
/// is 365 days from the request date"
/// (`smithy_query(query="doc",shape="ImportKeyMaterialRequest",member="ValidTo")`).
pub const MAX_IMPORTED_MATERIAL_VALIDITY_DAYS: u64 = 365;

/// What a caller stated about the key material it imported: whether it expires, when,
/// and the description it gave the material.
#[derive(Debug, Clone)]
pub struct ImportedKeyMaterial {
    /// An `ExpirationModelType` value — [`EXPIRATION_MODEL_EXPIRES`] or
    /// [`EXPIRATION_MODEL_DOES_NOT_EXPIRE`].
    pub expiration_model: String,
    /// When the material expires, which is present exactly when the expiration model
    /// says it expires: "this parameter is required when the value of the
    /// ExpirationModel parameter is KEY_MATERIAL_EXPIRES. Otherwise it is not valid"
    /// (`smithy_query(query="doc",shape="ImportKeyMaterialRequest",member="ValidTo")`).
    pub valid_to: Option<SystemTime>,
    /// "Description for the key material being imported", as the request gave it.
    pub description: Option<String>,
}

/// `KeyState` values, from `smithy_query(shape="KeyState")`.
pub const KEY_STATE_ENABLED: &str = "Enabled";
/// See [`KEY_STATE_ENABLED`].
pub const KEY_STATE_DISABLED: &str = "Disabled";
/// See [`KEY_STATE_ENABLED`].
pub const KEY_STATE_PENDING_DELETION: &str = "PendingDeletion";
/// The state of a key created with no key material, which waits for
/// `ImportKeyMaterial` to supply it. See [`KEY_STATE_ENABLED`].
pub const KEY_STATE_PENDING_IMPORT: &str = "PendingImport";

/// The TRANSIENT state a replica key of a multi-Region key set is in while KMS is
/// still creating it: "when this operation completes, the new replica key has a
/// transient key state of Creating. This key state changes to Enabled (or
/// PendingImport) after a few seconds when the process of creating the new replica
/// key is complete. While the key state is Creating, you can manage key, but you
/// cannot yet use it in cryptographic operations"
/// (`smithy_query(query="doc",shape="ReplicateKey")`). See [`KEY_STATE_ENABLED`].
pub const KEY_STATE_CREATING: &str = "Creating";

/// How long a replica key stays in [`KEY_STATE_CREATING`] before it reaches the
/// state it was created for.
///
/// The documentation says only "after a few seconds", so the length of the span is
/// this emulator's. It is deliberately short, so no caller waits on it, and
/// NON-ZERO, so a caller acting straight after the replicate still observes the
/// transient state and the key-state refusal that goes with it. The instant it ends
/// is stored on the key record ([`Cmk::creating_until`]) and the state a reader
/// reports is derived from the clock, so nothing has to be written when it passes.
pub const REPLICA_CREATING_SPAN: Duration = Duration::from_millis(500);

/// The TRANSIENT state the old and the new primary key of a multi-Region key set are
/// in while KMS is still moving the primary-key designation between them: "even after
/// this operation completes, the process of updating the primary Region might still be
/// in progress for a few more seconds. ... The old and new primary keys have a
/// transient key state of Updating. The original key state is restored when the update
/// is complete. While the key state is Updating, you can use the keys in cryptographic
/// operations, but you cannot replicate the new primary key or perform certain
/// management operations, such as enabling or disabling these keys"
/// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`). See
/// [`KEY_STATE_ENABLED`].
pub const KEY_STATE_UPDATING: &str = "Updating";

/// How long the old and the new primary key of a multi-Region key set stay in
/// [`KEY_STATE_UPDATING`] after the call that moved the designation between them.
///
/// The documentation says only "a few more seconds", so the length of the span is this
/// emulator's. It is NON-ZERO, so a caller racing or retrying the move still observes
/// the set while the move is in flight and the key-state refusal that goes with it.
/// The instant it ends is stored on the move itself and both the designation and the
/// designation and the key state a reader reports are derived from the clock, so
/// nothing has to be written when it passes — and the designation arrives exactly when
/// the transient state is over.
pub const PRIMARY_REGION_UPDATE_SPAN: Duration = Duration::from_secs(3);

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

/// The `KeyManagerType` of a key Amazon Web Services manages on a service's behalf:
/// "KMS keys in your Amazon Web Services account are either customer managed or
/// Amazon Web Services managed"
/// (`smithy_query(query="doc",shape="KeyMetadata",member="KeyManager")`), and this is
/// the value that tells the second kind apart. See [`KEY_MANAGER_CUSTOMER`].
pub const KEY_MANAGER_AWS: &str = "AWS";

/// The only `EncryptionAlgorithmSpec` a symmetric encryption key supports:
/// "The default value, SYMMETRIC_DEFAULT, is the algorithm used for symmetric
/// encryption KMS keys"
/// (`smithy_query(query="doc",shape="EncryptRequest",member="EncryptionAlgorithm")`).
pub const ENCRYPTION_ALGORITHM_SYMMETRIC_DEFAULT: &str = "SYMMETRIC_DEFAULT";

/// The `EncryptionAlgorithmSpec` of RSAES-OAEP with SHA-1, from
/// `smithy_query(query="shape",shape="EncryptionAlgorithmSpec")`: the OAEP scheme of
/// RFC 8017 with SHA-1 as both the message digest and the hash of MGF1, which is the
/// parameter set the published public half of an RSA KMS key encrypts under (see
/// [`decrypt_asymmetric`]).
pub const ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_1: &str = "RSAES_OAEP_SHA_1";

/// The `EncryptionAlgorithmSpec` of RSAES-OAEP with SHA-256, the other algorithm an
/// RSA KMS key advertises: "if you are using an asymmetric KMS key, we recommend
/// RSAES_OAEP_SHA_256"
/// (`smithy_query(query="doc",shape="EncryptRequest",member="EncryptionAlgorithm")`).
pub const ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_256: &str = "RSAES_OAEP_SHA_256";

/// The `RotationType` of a rotation a caller asked for, as opposed to `AUTOMATIC`,
/// from `smithy_query(shape="RotationType")`.
pub const ROTATION_TYPE_ON_DEMAND: &str = "ON_DEMAND";

/// How many on-demand rotations one KMS key allows: "You can perform on-demand key
/// rotation a maximum of 25 times per KMS key"
/// (`smithy_query(query="doc",shape="RotateKeyOnDemand")`).
pub const MAX_ON_DEMAND_ROTATIONS: usize = 25;

/// How long an on-demand rotation takes to COMPLETE, which is the span
/// `GetKeyRotationStatus` reports it as in progress for: "KMS uses a background process to
/// perform rotations. As a result, there might be a slight delay between initiating
/// on-demand key rotation and the rotation's completion. Once the on-demand rotation is
/// complete, KMS removes this field from the response"
/// (`smithy_query(query="doc",shape="GetKeyRotationStatusResponse",member="OnDemandRotationStartDate")`).
///
/// The documentation puts no number on that "slight delay", so the length of the span is
/// this emulator's, like [`REPLICA_CREATING_SPAN`]. It is deliberately short, so no caller
/// waits on it, and NON-ZERO, so a caller reading the status straight after the call still
/// observes the rotation it started — and until it elapses the key goes on encrypting
/// under the material it had, because that is what an in-progress rotation is.
pub const ON_DEMAND_ROTATION_SPAN: Duration = Duration::from_millis(300);

/// How long KMS's usage tracking takes to produce the record `GetKeyLastUsage`
/// reports: "the TrackingStartDate in the GetKeyLastUsage response indicates the date
/// from which KMS began recording cryptographic activity for a given key"
/// (`smithy_query(query="doc",shape="GetKeyLastUsage")`) — the recording is a pipeline
/// of its own and lags the call it records, which is why the recorded response of a
/// `GetKeyLastUsage` made straight after a cryptographic operation carries the usage
/// record EMPTY.
///
/// The documentation puts no number on that lag, so the length of the span is this
/// emulator's, like [`ON_DEMAND_ROTATION_SPAN`]. It is NON-ZERO, so a caller reading the
/// record straight after using the key still sees what AWS answers there — nothing
/// recorded yet — and it is a few seconds at most, so a caller polling for the record
/// gets it without waiting long.
pub const USAGE_TRACKING_LAG: Duration = Duration::from_secs(3);

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

/// [`MAX_ALIASES`], for custom key stores. A custom key store belongs to one Region
/// and the Region comes from the caller's own credential scope rather than from a
/// fixed set, so nothing else bounds how many records this store holds.
/// [`ComAmazonawsKmsTrentserviceState::insert_custom_key_store`] persists nothing
/// past it, and a create is to refuse past it with the modelled
/// `LimitExceededException` ("a length constraint or quota was exceeded") rather
/// than mint an id it cannot persist.
pub const MAX_CUSTOM_KEY_STORES: usize = 5_000;

/// Per-service state for com-amazonaws-kms-trentservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsKmsTrentserviceState {
    /// Every persisted key, keyed by its KEY ARN
    /// (`arn:aws:kms:<region>:<account>:key/<key-id>`) rather than by its key id:
    /// every key of a multi-Region set shares ONE key id — "these KMS keys have the
    /// same key ID, key material, and other metadata"
    /// (`smithy_query(query="doc",shape="ReplicateKey")`) — while each of them is a
    /// separate key living in its own Region, so the key id alone does not identify
    /// one. The ARN does, because the Region is part of it ([`key_arn`]).
    pub keys: HashMap<String, Cmk>,
    /// The key ARNs in creation order. `ListKeys` pages over this, so a page
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
    ///
    /// The map is ORDERED by that key, because grant id ascending is the order a
    /// grant reader reports grants in ([`Self::grants_in`]), so the order the store
    /// holds them in is the order they are served in and a page boundary is stable
    /// across calls.
    pub grants: BTreeMap<String, Grant>,
    /// Every persisted custom key store, keyed by its minted key store id. A custom
    /// key store is a regional resource, so each record carries the Region whose
    /// endpoint created it and a reader serves only its own
    /// ([`Self::custom_key_stores_in`]).
    pub custom_key_stores: HashMap<String, CustomKeyStore>,
    /// The custom key store ids in creation order, so a custom key store reader
    /// pages over a stable order rather than the hash map's.
    pub custom_key_store_order: Vec<String>,
    /// The wrapping key pairs `GetParametersForImport` has handed out and not yet
    /// seen expire, keyed by the import token that names each — see
    /// [`ImportParameters`]. `ImportKeyMaterial` is the only reader.
    pub import_parameters: HashMap<String, ImportParameters>,
}

impl ComAmazonawsKmsTrentserviceState {
    /// Persist a freshly minted key, remembering its place in creation order.
    ///
    /// The caller refuses the request past [`MAX_KEYS`] BEFORE minting, so this
    /// never grows the store without a bound.
    pub fn insert_key(&mut self, key: Cmk) {
        self.key_order.push(key.arn.clone());
        self.keys.insert(key.arn.clone(), key);
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

    /// Persist a freshly created grant.
    ///
    /// The caller refuses the request past [`MAX_GRANTS_PER_KEY`] BEFORE minting it,
    /// so this never grows the store without a bound.
    pub fn insert_grant(&mut self, grant: Grant) {
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

    /// The grants `region`'s endpoint holds for the key `key_id`, in the order a
    /// grant reader reports them — see [`Self::grants_in`].
    pub fn grants_for_key(&self, region: &str, key_id: &str) -> Vec<&Grant> {
        self.grants_in(region)
            .into_iter()
            .filter(|grant| grant.key_id == key_id)
            .collect()
    }

    /// Every grant `region`'s endpoint holds, ordered by GRANT ID ascending — the
    /// order a grant reader reports grants in, whichever selection it reports, so
    /// two readers of the same grants agree and a page boundary is stable across
    /// calls. A grant reader that is not scoped to one key pages over this.
    ///
    /// The order is a property of the grants themselves rather than of when each was
    /// written, so a steady-state grant set reads the same however it was built, and
    /// filtering it keeps the order.
    pub fn grants_in(&self, region: &str) -> Vec<&Grant> {
        self.grants
            .values()
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
    /// are gone from the next request on, and a later reader cannot page over a grant
    /// the store no longer holds.
    pub fn remove_grant(&mut self, region: &str, grant_id: &str) -> Option<Grant> {
        self.grant(region, grant_id)?;
        self.grants.remove(grant_id)
    }

    /// Whether the store can hold another custom key store, across every Region,
    /// which is what a create checks before it mints an id. See
    /// [`Self::has_room_for_another_alias`] for why a per-Region bound is not enough.
    pub fn has_room_for_another_custom_key_store(&self) -> bool {
        self.custom_key_stores.len() < MAX_CUSTOM_KEY_STORES
    }

    /// Persist a freshly created custom key store, remembering its place in
    /// creation order, and report whether it was persisted.
    ///
    /// `false` means the store already holds [`MAX_CUSTOM_KEY_STORES`] and NOTHING
    /// was written. The bound is enforced HERE as well as by the caller that refuses
    /// the request, so no sequence of requests can grow this map without a ceiling.
    pub fn insert_custom_key_store(&mut self, store: CustomKeyStore) -> bool {
        if !self.has_room_for_another_custom_key_store() {
            return false;
        }
        self.custom_key_store_order
            .push(store.custom_key_store_id.clone());
        self.custom_key_stores
            .insert(store.custom_key_store_id.clone(), store);
        true
    }

    /// The custom key stores `region`'s endpoint serves, in creation order — what a
    /// custom key store reader pages over, so a page boundary is stable across
    /// calls.
    ///
    /// "You cannot perform this operation on a custom key store in a different
    /// Amazon Web Services account"
    /// (`smithy_query(query="doc",shape="DescribeCustomKeyStores")`), and a custom
    /// key store belongs to one Region, so a key store created through another
    /// Region's endpoint is not served here.
    pub fn custom_key_stores_in(&self, region: &str) -> Vec<&CustomKeyStore> {
        self.custom_key_store_order
            .iter()
            .filter_map(|id| self.custom_key_stores.get(id))
            .filter(|store| store.region == region)
            .collect()
    }

    /// The custom key store `region`'s endpoint serves under exactly the id
    /// `custom_key_store_id`.
    ///
    /// A `CustomKeyStoreId` member has ONE form — the id of a key store this account
    /// and Region holds — so a value that is not one of those ids names no key store,
    /// whatever it looks like: "KMS cannot find a custom key store with the specified
    /// key store name or ID" (`CustomKeyStoreNotFoundException`) is the one answer for
    /// a well-formed-but-absent id and for a value that is not a key store id at all.
    /// One resolver for every operation that takes the member, so no two of them split
    /// that absence differently.
    pub fn custom_key_store_in(
        &self,
        region: &str,
        custom_key_store_id: &str,
    ) -> Option<&CustomKeyStore> {
        self.custom_key_stores
            .get(custom_key_store_id)
            .filter(|store| store.region == region)
    }

    /// [`Self::custom_key_store_in`], for an operation that then mutates the key store
    /// — moving it through its connection state, or changing its properties.
    ///
    /// A custom key store belongs to one Region, so an id minted through another
    /// Region's endpoint names no key store here: the same Region scoping
    /// [`Self::custom_key_stores_in`] reports under.
    pub fn custom_key_store_mut(
        &mut self,
        region: &str,
        custom_key_store_id: &str,
    ) -> Option<&mut CustomKeyStore> {
        self.custom_key_stores
            .get_mut(custom_key_store_id)
            .filter(|store| store.region == region)
    }

    /// Materialise every custom-key-store connection change the clock has already
    /// made due ([`CustomKeyStore::settle`]).
    ///
    /// Every operation that reports or moves a connection state does this first, so
    /// the state it works from is the one the clock owes rather than the transient
    /// one the call that started the change left behind.
    pub fn settle_custom_key_stores(&mut self) {
        let now = SystemTime::now();
        for store in self.custom_key_stores.values_mut() {
            store.settle(now);
        }
    }

    /// The key a `KeyId` request member names, accepting every form KMS does:
    /// a raw key id, a key ARN, `alias/<name>` and an alias ARN.
    ///
    /// `region` is the Region the request was signed for. A KMS key is a REGIONAL
    /// resource, so a key created through one Region's endpoint is not visible
    /// through another's.
    pub fn resolve(&self, region: &str, id: &str) -> KeyRef<'_> {
        match parse_key_identifier(id) {
            Err(IdentifierRefusal::MalformedArn) => KeyRef::MalformedArn,
            Err(IdentifierRefusal::UnauthorizedAccount) => KeyRef::UnauthorizedAccount,
            Ok(identifier) => match self.lookup(region, &identifier) {
                Some(key) => KeyRef::Found(key),
                None => KeyRef::NotFound,
            },
        }
    }

    /// [`Self::resolve`], for an operation that then mutates the key.
    pub fn resolve_mut(&mut self, region: &str, id: &str) -> KeyRefMut<'_> {
        match parse_key_identifier(id) {
            Err(IdentifierRefusal::MalformedArn) => KeyRefMut::MalformedArn,
            Err(IdentifierRefusal::UnauthorizedAccount) => KeyRefMut::UnauthorizedAccount,
            Ok(identifier) => {
                match self.lookup(region, &identifier).map(|key| key.arn.clone()) {
                    Some(arn) => match self.keys.get_mut(&arn) {
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
        self.keys.get(&key_arn(region, key_id))
    }

    /// [`Self::key_in`], for a caller that then mutates the key it found.
    pub fn key_mut_in(&mut self, region: &str, key_id: &str) -> Option<&mut Cmk> {
        self.keys.get_mut(&key_arn(region, key_id))
    }

    /// The key ARNs of every key of the multi-Region key set `key_id` holds — one
    /// per Region that holds a key of the set, since every key of a set shares the
    /// set's key id.
    fn set_member_arns(&self, key_id: &str) -> Vec<String> {
        self.keys
            .values()
            .filter(|key| key.key_id == key_id)
            .map(|key| key.arn.clone())
            .collect()
    }

    /// Record that `region` now holds a replica key of the multi-Region key set
    /// `key_id`, on EVERY key of that set.
    ///
    /// Which Regions hold the set's keys is a property its members share: "KMS
    /// automatically synchronizes these shared properties among related multi-Region
    /// keys" (`smithy_query(query="doc",shape="ReplicateKey")`), so the membership is
    /// written through every member rather than only through the key the request
    /// named — and a reader at any of their endpoints reports the same set.
    pub fn record_replica_region(&mut self, key_id: &str, region: &str) {
        for arn in self.set_member_arns(key_id) {
            if let Some(topology) = self
                .keys
                .get_mut(&arn)
                .and_then(|key| key.multi_region_topology.as_mut())
            {
                topology.add_replica_region(region.to_string());
            }
        }
    }

    /// Configure the automatic rotation schedule of EVERY key of the multi-Region key
    /// set `key_id`, from the rotation period the request named.
    ///
    /// The automatic rotation setting is a property the keys of a set share: "to enable
    /// or disable automatic rotation of a set of related multi-Region keys, set the
    /// property on the primary key" and "KMS automatically synchronizes these shared
    /// properties among related multi-Region keys"
    /// (`smithy_query(query="doc",shape="EnableKeyRotation")`,
    /// `smithy_query(query="doc",shape="ReplicateKey")`) — so the one call that
    /// configures it writes the SAME schedule through every member, and the status each
    /// member's own Region reports is that one schedule rather than whatever the member
    /// carried before.
    ///
    /// A key that belongs to no set is the only key of its key id, so this writes it
    /// and nothing else.
    pub fn schedule_rotation_across_set(&mut self, key_id: &str, period_in_days: Option<i32>) {
        let schedule = RotationSchedule::starting_now(period_in_days);
        for arn in self.set_member_arns(key_id) {
            if let Some(key) = self.keys.get_mut(&arn) {
                key.rotation_schedule = Some(schedule.clone());
            }
        }
    }

    /// Stop automatic rotation on EVERY key of the multi-Region key set `key_id` — the
    /// other end of [`Self::schedule_rotation_across_set`], for the same reason: the
    /// setting is one the set's keys share.
    pub fn unschedule_rotation_across_set(&mut self, key_id: &str) {
        for arn in self.set_member_arns(key_id) {
            if let Some(key) = self.keys.get_mut(&arn) {
                key.unschedule_rotation();
            }
        }
    }

    /// Make the key in `region` the primary key of the multi-Region key set `key_id`
    /// ([`MultiRegionTopology::promote`]) on every key of that set, answering whether
    /// `region` held a replica of it to promote at all.
    ///
    /// See [`Self::record_replica_region`] for why every member is written.
    ///
    /// The move is not instantaneous: "even after this operation completes, the process
    /// of updating the primary Region might still be in progress for a few more
    /// seconds. ... The old and new primary keys have a transient key state of
    /// Updating. The original key state is restored when the update is complete"
    /// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`), so the move is
    /// recorded with the instant the clock makes it due
    /// ([`PRIMARY_REGION_UPDATE_SPAN`]) rather than taking effect at once: until that
    /// instant the set reports the designation it still has and a second move of it is
    /// a key-state refusal ([`Self::primary_region_update_in_flight`]), and from it the
    /// designation has moved and the transient state is over — ONE instant, so a reader
    /// that can see the move complete is never refused for it being in flight.
    pub fn promote_primary_region(&mut self, key_id: &str, region: &str) -> bool {
        let due = SystemTime::now() + PRIMARY_REGION_UPDATE_SPAN;
        let mut promoted = false;
        for arn in self.set_member_arns(key_id) {
            if let Some(topology) = self
                .keys
                .get_mut(&arn)
                .and_then(|key| key.multi_region_topology.as_mut())
            {
                promoted |= topology.promote(region, due);
            }
        }
        promoted
    }

    /// Whether the clock still owes the multi-Region key set `key_id` a primary-Region
    /// move a call has already started — the set is transiently busy moving its primary
    /// key, so a management operation on the SET is not one it is in a valid state for.
    ///
    /// The judgement is set-wide rather than per key, because it is the set's single
    /// primary-key designation that is moving: whichever of its keys a request names, a
    /// promotion it asks for is serialized behind the one in flight.
    pub fn primary_region_update_in_flight(&self, key_id: &str) -> bool {
        self.keys.values().any(|key| {
            key.key_id == key_id
                && key
                    .multi_region_topology
                    .as_ref()
                    .is_some_and(MultiRegionTopology::promotion_in_flight)
        })
    }

    /// Record that `operation` used the key `key_id` that `region`'s endpoint serves
    /// in a successful cryptographic operation — see [`Cmk::record_last_usage`], which
    /// this is the store-level way in to, for a handler that has finished reading the
    /// key.
    pub fn record_last_usage(&mut self, region: &str, key_id: &str, operation: &str) {
        if let Some(key) = self.key_mut_in(region, key_id) {
            key.record_last_usage(operation);
        }
    }

    /// Remember one wrapping key pair against the import token that names it,
    /// dropping every record whose validity the clock has already ended.
    ///
    /// The pruning is what the clock already owes: a pair past its
    /// `ParametersValidTo` can no longer be used by anyone, so holding it changes no
    /// answer. The caller refuses the request past [`MAX_IMPORT_PARAMETERS`] BEFORE
    /// minting, so this never grows the store without a bound.
    pub fn insert_import_parameters(&mut self, token: String, parameters: ImportParameters) {
        let now = SystemTime::now();
        self.import_parameters
            .retain(|_, held| held.usable_at(now));
        self.import_parameters.insert(token, parameters);
    }

    /// How many wrapping key pairs are still usable at `now`, which is what
    /// [`MAX_IMPORT_PARAMETERS`] bounds.
    pub fn usable_import_parameters(&self, now: SystemTime) -> usize {
        self.import_parameters
            .values()
            .filter(|held| held.usable_at(now))
            .count()
    }

    /// The Amazon Web Services managed key the predefined alias `alias_name`
    /// resolves to at `region`'s endpoint, associating the alias with a freshly
    /// created key the first time one is asked for.
    ///
    /// A predefined Amazon Web Services alias is a name that exists in every account
    /// and Region while carrying no target key — "the response might also include
    /// aliases that have no `TargetKeyId` field. These are predefined aliases that
    /// Amazon Web Services has created but has not yet associated with a KMS key"
    /// (`smithy_query(query="doc",shape="ListAliases")`) — and the call that asks for
    /// one is what gives it a key: "if you specify a predefined Amazon Web Services
    /// alias (an Amazon Web Services alias with no key ID), KMS associates the alias
    /// with an Amazon Web Services managed key and returns its KeyId and Arn in the
    /// response"
    /// (`smithy_query(query="doc",shape="DescribeKeyRequest",member="KeyId")`). So the
    /// association happens once and every later call resolves the alias to the same
    /// key, in that Region.
    ///
    /// `None` when `alias_name` is not a predefined Amazon Web Services alias
    /// ([`is_predefined_aws_alias`]), when the platform refused to produce key
    /// material, and when the store is already at its bound ([`MAX_KEYS`],
    /// [`MAX_ALIASES`]) — so a caller naming ever more predefined aliases cannot grow
    /// the store without end.
    pub fn associate_predefined_aws_alias(
        &mut self,
        region: &str,
        alias_name: &str,
    ) -> Option<&Cmk> {
        if !is_predefined_aws_alias(alias_name) {
            return None;
        }
        // Already associated: the alias names its key, and nothing is created.
        if let Some(key_id) = self
            .alias(region, alias_name)
            .map(|alias| alias.target_key_id.clone())
        {
            return self.key_in(region, &key_id);
        }
        if !self.has_room_for_another_key() || !self.has_room_for_another_alias() {
            return None;
        }
        // An Amazon Web Services managed key is a symmetric encryption key whose
        // material KMS generated, so its material is the material that key spec
        // calls for rather than a kind named here.
        let material = mint_material(key_spec_profile(KEY_SPEC_SYMMETRIC_DEFAULT)?)?;
        let key = Cmk::aws_managed(mint_key_id(false), region.to_string(), material);
        let key_id = key.key_id.clone();
        self.insert_key(key);
        self.insert_alias(Alias::new(
            region.to_string(),
            alias_name.to_string(),
            key_id.clone(),
        ));
        self.key_in(region, &key_id)
    }

    /// The key ids `region`'s endpoint serves, in creation order.
    pub fn key_ids_in(&self, region: &str) -> Vec<&str> {
        self.key_order
            .iter()
            .filter_map(|arn| self.keys.get(arn))
            .filter(|key| key.region == region)
            .map(|key| key.key_id.as_str())
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
    /// The identifier is a KMS ARN whose account-id segment is not an account id —
    /// see [`names_an_account_id`]. The request is refused for want of permission,
    /// before the resource it names is looked up.
    UnauthorizedAccount,
}

/// [`KeyRef`], for a mutating operation.
pub enum KeyRefMut<'a> {
    /// The identifier named this key.
    Found(&'a mut Cmk),
    /// The identifier was well formed but names no key in this account and Region.
    NotFound,
    /// The identifier is an ARN that is not a well-formed KMS key or alias ARN.
    MalformedArn,
    /// See [`KeyRef::UnauthorizedAccount`].
    UnauthorizedAccount,
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

/// Why a `KeyId` member names nothing this endpoint can act on, short of naming a
/// resource that does not exist.
enum IdentifierRefusal {
    /// The identifier is an ARN that is not a well-formed KMS key or alias ARN.
    MalformedArn,
    /// The ARN has the shape of a KMS ARN, but its account-id segment is not an
    /// account id at all.
    UnauthorizedAccount,
}

/// Whether `account` has the form of an Amazon Web Services account id: exactly 12
/// decimal digits, which is `AccountIdType`'s modelled `@length(12, 12)` and
/// `@pattern(^[0-9]{12}$)` (`smithy_query(query="shape",shape="AccountIdType")`).
///
/// A value outside that form names no account, so no account's permission can cover a
/// resource addressed through it — which is why an ARN carrying one is refused for
/// want of permission rather than reported as a missing resource.
pub fn names_an_account_id(account: &str) -> bool {
    account.len() == ACCOUNT_ID_LEN && account.bytes().all(|byte| byte.is_ascii_digit())
}

/// How many digits an account id has, from `AccountIdType`'s modelled length.
const ACCOUNT_ID_LEN: usize = 12;

/// Parse a `KeyId` member into the thing it names.
///
/// The three steps are ordered the way KMS resolves an ARN: its SHAPE as a KMS key or
/// alias ARN, then AUTHORIZATION of the account it names, and only then — in
/// [`ComAmazonawsKmsTrentserviceState::lookup`] — the existence of the resource. So an
/// ARN whose account-id segment is not an account id is refused for want of permission
/// whatever it goes on to name, and a cross-account ARN never discloses whether the
/// resource it names exists.
fn parse_key_identifier(id: &str) -> Result<KeyIdentifier, IdentifierRefusal> {
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
        return Err(IdentifierRefusal::MalformedArn);
    };
    if partition.is_empty() || *service != ARN_SERVICE || region.is_empty() || account.is_empty() {
        return Err(IdentifierRefusal::MalformedArn);
    }
    let scope = Some(((*region).to_string(), (*account).to_string()));
    let identifier = if let Some(name) = resource.strip_prefix(ALIAS_PREFIX) {
        if name.is_empty() {
            return Err(IdentifierRefusal::MalformedArn);
        }
        KeyIdentifier::Alias {
            alias: format!("{ALIAS_PREFIX}{name}"),
            scope,
        }
    } else {
        match resource.strip_prefix("key/") {
            Some(key_id) if !key_id.is_empty() => KeyIdentifier::KeyId {
                key_id: key_id.to_string(),
                scope,
            },
            _ => return Err(IdentifierRefusal::MalformedArn),
        }
    };
    if !names_an_account_id(account) {
        return Err(IdentifierRefusal::UnauthorizedAccount);
    }
    Ok(identifier)
}

/// How a failed `KeyId` lookup is reported: by the ARN of the resource the REQUEST
/// named, or — for a value that is no key identifier at all — by that value itself.
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
/// AND BEFORE the ARN is built, what the identifier names: a key id KMS could have
/// issued ([`names_a_key_id`]) names a key this endpoint could have held, so it is
/// reported by the key ARN it would have; a value in no key-id form names no key KMS
/// ever issued, so there is no such ARN and none is synthesised
/// ([`invalid_key_id_message`]).
///
/// `region` is the Region the request was signed for, and the Region of the ARN this
/// builds, since a KMS key is a regional resource and the same key id in another
/// Region is a different (absent) key. The exact sentences are examples of KMS's;
/// what this function pins is WHICH resource, in which form, the message names.
///
/// An identifier that is an ARN but not a KMS key or alias ARN has no resource ARN to
/// name, so the refusal names the identifier itself, with the same `Invalid arn …`
/// wording the Region case carries. The operations that model `InvalidArnException`
/// answer that instead and never reach this case; `Encrypt`, `Decrypt`,
/// `GetKeyLastUsage` and the tagging operations, which report such an identifier as not
/// found, do.
pub fn key_not_found_message(region: &str, requested: &str) -> String {
    if let Some(arn_region) = arn_region(requested) {
        if arn_region != region {
            return not_a_key_arn_message(arn_region.as_str());
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
            // A value that is not a key id KMS could have issued names no key it ever
            // had, so there is no key ARN to report it by: it is reported as an invalid
            // key id instead, echoing the identifier the request gave.
            if !names_a_key_id(&key_id) {
                return invalid_key_id_message(requested);
            }
            let resource = format!("key/{key_id}");
            format!(
                "Key '{}' does not exist",
                arn_of(region, scope, requested, &resource)
            )
        }
        // An identifier that is an ARN this endpoint cannot resolve has no resource
        // ARN to name, so the refusal names the ARN itself — the same "Invalid arn
        // <the part that is invalid>" wording the Region case above carries, where
        // the invalid part is the whole identifier rather than its Region.
        Err(_) => not_a_key_arn_message(requested),
    }
}

/// How KMS reports a `KeyId` that is not a key identifier at all: `Invalid keyId '<the
/// identifier>'`, with the identifier echoed exactly as the request supplied it.
///
/// It is the OTHER refusal a failed `KeyId` lookup answers, beside
/// [`key_not_found_message`]'s "Key '<key ARN>' does not exist": a well-formed key id
/// names a key this endpoint could have held and so is reported by the key ARN it would
/// have, while a value that is no key id ([`names_a_key_id`]) names no key KMS ever
/// issued — there is no such ARN to build, and none is invented, so the caller sees the
/// value it sent and nothing synthesised from it.
///
/// One function for the whole service, so no two operations word it differently.
pub fn invalid_key_id_message(requested: &str) -> String {
    format!("Invalid keyId '{requested}'")
}

/// How KMS reports an identifier it could not read as the ARN the request needed:
/// "Invalid arn <the identifier>".
///
/// It is the wording [`key_not_found_message`] gives an ARN this endpoint cannot resolve,
/// and the one `RetireGrant` reports a `KeyId` that is not a key ARN with — that member is
/// "the key ARN KMS key associated with the grant"
/// (`smithy_query(query="doc",shape="RetireGrantRequest",member="KeyId")`), so an
/// identifier in any other form names no key for it. One function for the whole service,
/// so no two operations word it differently.
pub fn not_a_key_arn_message(requested: &str) -> String {
    format!("Invalid arn {requested}")
}

/// Whether `key_id` has the form of a key id KMS could have issued: the hyphenated
/// 8-4-4-4-12 hexadecimal text of a UUID for a single-Region key, and
/// [`MULTI_REGION_KEY_ID_PREFIX`] followed by 32 hexadecimal characters for a
/// multi-Region one — the two forms [`mint_key_id`] produces, so a value in neither is
/// one no key of this service was ever given.
///
/// It is what separates the two refusals a failed `KeyId` lookup answers: a well-formed
/// id names a key this endpoint could have held ([`key_not_found_message`]), while a value
/// that is no key id at all is reported as an invalid one
/// ([`invalid_key_id_message`]). The version nibble of a UUID is deliberately NOT
/// required: KMS reads the TEXT FORM of the identifier, not which UUID version produced
/// it. That form is checked here rather than through `uuid`'s own parse, which also
/// accepts the hyphen-less and `urn:` spellings of the same value — forms a `KeyId` of a
/// key this service minted never takes.
pub fn names_a_key_id(key_id: &str) -> bool {
    if let Some(hex) = key_id.strip_prefix(MULTI_REGION_KEY_ID_PREFIX) {
        return hex.len() == MULTI_REGION_KEY_ID_HEX_LEN && is_hex(hex);
    }
    let groups: Vec<&str> = key_id.split('-').collect();
    groups.len() == UUID_GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_LENGTHS)
            .all(|(group, len)| group.len() == len && is_hex(group))
}

/// The lengths of the hexadecimal groups of a UUID's hyphenated text form, which is the
/// form of a single-Region key id ([`mint_key_id`]).
const UUID_GROUP_LENGTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// How many hexadecimal characters follow [`MULTI_REGION_KEY_ID_PREFIX`] in a
/// multi-Region key id ([`mint_key_id`]).
const MULTI_REGION_KEY_ID_HEX_LEN: usize = 32;

/// Whether every character of `text` is a hexadecimal digit, in either case: a key id is
/// minted in lowercase, but which case a caller sends an identifier in is not what makes
/// it a key id.
fn is_hex(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Whether `identifier` is the KEY ARN of a KMS key:
/// `arn:<partition>:kms:<region>:<account>:key/<key id>`.
///
/// Read through the tree's one ARN parse ([`::service_common::arn::parse`]) rather than a
/// second grammar here. It is what an operation whose `KeyId` member documents the key ARN
/// alone requires: a bare key id, an alias and an alias ARN each name no key for it.
pub fn names_a_key_arn(identifier: &str) -> bool {
    ::service_common::arn::parse(identifier).is_some_and(|arn| {
        arn.vendor == ARN_SERVICE
            && arn
                .resource
                .strip_prefix("key/")
                .is_some_and(|key_id| !key_id.is_empty())
    })
}

/// How a request that pins a ciphertext to a KMS key other than the one that sealed it
/// is refused: "the KeyId in a Decrypt request and the SourceKeyId in a ReEncrypt
/// request must identify the same KMS key that was used to encrypt the ciphertext"
/// (`smithy_query(query="doc",shape="IncorrectKeyException")`).
///
/// KMS words this refusal as entirely FIXED text that names neither key: the identifier
/// the request gave is the whole of what is wrong with it, and the key that did seal the
/// ciphertext is not disclosed to a caller who never named it.
///
/// One constant for the whole service, so `Decrypt` and `ReEncrypt` cannot word the same
/// refusal differently.
pub const INCORRECT_KEY_MESSAGE: &str =
    "The key ID in the request does not identify a CMK that can perform this operation.";

/// How a request addressing a resource through an ARN that names no account is
/// refused: KMS answers an authorization failure, and words it the way every AWS
/// service words one — naming the principal it refused and the IAM action it refused.
///
/// The principal is [`caller_identity_arn`]: every request to this emulator
/// authenticates as the one identity it publishes, so that is who the refusal is about.
///
/// The resource is NOT named. Authorization is evaluated before the identifier's
/// contents are, so a refusal on this path cannot report anything about the resource —
/// not even that it exists — and says so: the resource may not exist in this Region, or
/// no policy may allow or a policy may deny the action. One function for the whole
/// service, so no two operations word it differently.
pub fn unauthorized_account_message(operation: &str) -> String {
    unauthorized_resource_message(&caller_identity_arn(), operation)
}

/// [`unauthorized_account_message`], naming the principal the request actually
/// authenticated as ([`authorization::caller`]) rather than the identity this emulator
/// publishes.
///
/// It is the SAME sentence, so the two cannot drift apart: a refusal that cannot report
/// anything about the resource — whether it exists, whether a policy allows it, whether
/// one denies it — says exactly that, whoever was refused.
pub fn unauthorized_resource_message(principal: &str, operation: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {ARN_SERVICE}:{operation} on this \
         resource because the resource does not exist in this Region, no resource-based \
         policies allow access, or a resource-based policy explicitly denies access"
    )
}

/// The ARN of the identity this emulator's callers authenticate as, which is what an
/// authorization refusal names: the identity `sts:GetCallerIdentity` answers with for
/// the credentials every request here is signed with.
pub fn caller_identity_arn() -> String {
    ::engine::emulator_arn("iam", "", "user/emulator")
}

/// The refusal every operation answers for [`KeyRef::UnauthorizedAccount`].
///
/// `AccessDeniedException` is not in this service's model at all — KMS answers an
/// authorization failure with it regardless — so it travels on the error channel's
/// own variant and is built here once, for every operation's error type.
pub fn unauthorized_account<E>(operation: &str) -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::access_denied(unauthorized_account_message(operation))
}

/// How KMS words its refusal of an encryption context that is not a legal encryption
/// context at all: fixed text that names neither the entry nor the value, because what
/// the request got wrong is the context as a whole.
pub const INVALID_ENCRYPTION_CONTEXT_MESSAGE: &str = "Invalid EncryptionContext";

/// Whether `context` is an encryption context KMS accepts, and the refusal that reports
/// it when it is not.
///
/// An encryption context is "a collection of non-secret key-value pairs"
/// (`smithy_query(query="doc",shape="EncryptRequest",member="EncryptionContext")`), so
/// an entry whose name or whose value is the empty string binds nothing: the REQUEST is
/// malformed, and it is refused at input validation — a `ValidationException` carrying
/// [`INVALID_ENCRYPTION_CONTEXT_MESSAGE`] — before any ciphertext is authenticated
/// against it.
///
/// That ordering is the whole point of judging it here. A context that is well formed
/// but not the one a ciphertext was sealed under is an AUTHENTICATION failure, which
/// `InvalidCiphertextException` reports and which only an operation that opens a
/// ciphertext models at all; a context that is not a context is wrong on every
/// operation that takes one, including the ones that merely seal, where there is no
/// ciphertext to authenticate against and no such fault is modelled.
///
/// Generic over the error channel, so every operation taking an encryption context
/// refuses one identically.
pub fn check_encryption_context<E>(
    context: Option<&HashMap<String, String>>,
) -> Result<(), crate::scaffolded::Scaffolded<E>> {
    let well_formed = context.is_none_or(|context| {
        context
            .iter()
            .all(|(name, value)| !name.is_empty() && !value.is_empty())
    });
    if well_formed {
        return Ok(());
    }
    Err(crate::scaffolded::Scaffolded::validation(
        INVALID_ENCRYPTION_CONTEXT_MESSAGE,
    ))
}

/// How KMS refuses a request to open a ciphertext whose key material is not material
/// this endpoint can authorize: it does not say whether that key exists, because the
/// answer would disclose a key of another Region to a caller this endpoint cannot
/// authorize for it.
///
/// Single-Region key material does not travel: a ciphertext minted in one Region, or a
/// key ARN of one, presented to ANOTHER Region's endpoint names material this endpoint
/// holds no key for — which is an authorization decision rather than a lookup, so the
/// sentence covers all three of the reasons it can be refused at once.
pub const FOREIGN_KEY_MATERIAL_MESSAGE: &str = "The ciphertext refers to a customer \
     master key that does not exist, does not exist in this region, or you are not \
     allowed to access.";

/// The refusal every operation that OPENS a ciphertext answers for key material this
/// endpoint cannot authorize ([`FOREIGN_KEY_MATERIAL_MESSAGE`]).
///
/// `AccessDeniedException` is not in this service's model at all — KMS answers an
/// authorization failure with it regardless — so it travels on the error channel's own
/// variant and is built here once, for every operation's error type.
pub fn foreign_key_material<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::access_denied(FOREIGN_KEY_MATERIAL_MESSAGE)
}

/// Whether `identifier` is an ARN whose Region segment is not `region`, the Region the
/// request was signed for.
///
/// Such an identifier addresses a resource of another Region's endpoint, which this one
/// can never serve — the same reading [`key_not_found_message`] gives it, here as a
/// question an operation can ask before it decides which refusal that is.
///
/// The Region comes from the ARN's own field ([`arn_region`]), so an ARN of another Region
/// is judged foreign whatever its resource part says: which endpoint can serve an
/// identifier is settled before what that identifier names is read at all. That is why
/// `Decrypt` and `ReEncrypt`, which ask this of a `KeyId` naming the key a ciphertext was
/// sealed under, answer the key-material authorization refusal for a malformed ARN of
/// another Region too — the endpoint cannot authorize any key of that Region, so there is
/// nothing to report as missing.
pub fn names_another_region(region: &str, identifier: &str) -> bool {
    arn_region(identifier).is_some_and(|arn_region| arn_region != region)
}

/// The Region segment of `identifier`, when it is an ARN in this service's namespace
/// that names a Region at all.
///
/// Read from the ARN's six FIELDS ([`::service_common::arn::parse`]) rather than from a
/// resolved key or alias identifier, because which Region an ARN addresses is settled by
/// the field itself: an ARN whose resource part names no key or alias still pins the
/// request to the Region it carries, and that Region is what the refusal reports. A
/// value of another vendor addresses nothing of this service's, and an ARN written for a
/// partition-global resource names no Region, so neither reports one.
fn arn_region(identifier: &str) -> Option<String> {
    let arn = ::service_common::arn::parse(identifier)?;
    (arn.vendor == ARN_SERVICE && !arn.region.is_empty()).then(|| arn.region.to_string())
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

/// The alias name (`alias/<name>`) a `KeyId`-shaped identifier names at `region`'s
/// endpoint, in either form KMS accepts for an alias: bare `alias/<name>`, or an
/// alias ARN.
///
/// `None` when the identifier names a key rather than an alias, when it is an ARN
/// that is no KMS key or alias ARN, and when it is an alias ARN whose Region or
/// account segment names another endpoint's alias — an ARN addresses one alias in
/// one Region of one account, exactly as it does one key.
pub fn alias_named_by(region: &str, identifier: &str) -> Option<String> {
    let Ok(KeyIdentifier::Alias { alias, scope }) = parse_key_identifier(identifier) else {
        return None;
    };
    match scope {
        Some((arn_region, account))
            if arn_region != region || account != ::engine::emulator_account_id() =>
        {
            None
        }
        _ => Some(alias),
    }
}

/// Whether `alias_name` is a PREDEFINED Amazon Web Services alias: a name Amazon Web
/// Services itself holds in every account and Region for the Amazon Web Services
/// managed key of one service, rather than a name a caller created.
///
/// "You can recognize Amazon Web Services aliases because their names have the format
/// `aws/<service-name>`, such as `aws/dynamodb`"
/// (`smithy_query(query="doc",shape="ListAliases")`), so the NAMESPACE is the rule:
/// [`ALIAS_PREFIX_AWS_MANAGED`] followed by one non-empty service-name segment. No
/// catalogue of service names is written here — the model documents the form, and a
/// list of the services that have such a key is not in it, so a list would be
/// invented rather than read.
///
/// It is the exact complement of what [`validate_alias_name`] refuses a caller: a
/// name under this prefix is never one a `CreateAlias` created.
pub fn is_predefined_aws_alias(alias_name: &str) -> bool {
    let Some(service_name) = alias_name.strip_prefix(ALIAS_PREFIX_AWS_MANAGED) else {
        return false;
    };
    !service_name.is_empty()
        && service_name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Why an alias name is not one a caller may create.
///
/// Three rules, all from the member's own documentation
/// (`smithy_query(query="doc",shape="CreateAliasRequest",member="AliasName")`): the
/// value "must begin with alias/ followed by a name", so the prefix is required and
/// what follows it may not be empty; and "the alias name cannot begin with
/// alias/aws/", which is reserved for AWS managed keys.
///
/// They are kept APART because KMS answers them differently: a name that is not an
/// alias name at all is a malformed request, while a name under the reserved
/// namespace is a well-formed name that is not the caller's to take. A caller
/// branching on the error code is what that distinction is for, so the classification
/// lives here and each operation maps it onto its own error channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AliasNameRefusal {
    /// The name does not begin with `alias/`, so it is not an alias name.
    MissingPrefix,
    /// The name is the prefix with no name after it.
    EmptyName,
    /// The name is under the `alias/aws/` namespace, which is reserved.
    Reserved,
}

impl AliasNameRefusal {
    /// How KMS words this refusal.
    ///
    /// Every sentence is FIXED text: what the request got wrong is the name as a
    /// whole, which the caller already holds, so none of the three interpolates the
    /// rejected value.
    pub fn message(self) -> String {
        match self {
            Self::MissingPrefix => format!(
                "Alias must start with the prefix \"{ALIAS_PREFIX}\". Please see \
                 {ALIAS_GUIDE_URL}"
            ),
            Self::EmptyName => {
                format!("Alias must have a name after the prefix \"{ALIAS_PREFIX}\".")
            }
            // Fixed text, naming neither the alias nor the key: the namespace is the
            // whole of what the request got wrong.
            Self::Reserved => {
                "The alias name for a customer managed CMK cannot begin with alias/aws/."
                    .to_string()
            }
        }
    }
}

/// Where KMS points a caller whose alias name is not an alias name: the aliases page
/// of the Key Management Service Developer Guide, which the `AliasName` member's own
/// documentation names as the reference for the rule.
const ALIAS_GUIDE_URL: &str = "https://docs.aws.amazon.com/kms/latest/developerguide/kms-alias.html";

/// Whether `alias_name` is a name a caller may create, and why not when it is not.
///
/// The character set and the 1..256 length are modelled, so the boundary validator has
/// already refused anything outside them before this is reached; what is left are the
/// rules of [`AliasNameRefusal`].
///
/// One function for the whole service, so no two alias operations judge the same name
/// differently.
pub fn validate_alias_name(alias_name: &str) -> Result<(), AliasNameRefusal> {
    let Some(name) = alias_name.strip_prefix(ALIAS_PREFIX) else {
        return Err(AliasNameRefusal::MissingPrefix);
    };
    if name.is_empty() {
        return Err(AliasNameRefusal::EmptyName);
    }
    if alias_name.starts_with(ALIAS_PREFIX_AWS_MANAGED) {
        return Err(AliasNameRefusal::Reserved);
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

// ---------------------------------------------------------------------------
// Custom key stores
// ---------------------------------------------------------------------------

/// The `ConnectionStateType` of a custom key store that is not connected to its
/// backing key store: "The `ConnectionState` value is `DISCONNECTED` only if the
/// key store has never been connected or you use the DisconnectCustomKeyStore
/// operation to disconnect it"
/// (`smithy_query(query="doc",shape="CustomKeyStoresListEntry",member="ConnectionState")`),
/// so it is the state a freshly created key store is in.
pub const CONNECTION_STATE_DISCONNECTED: &str = "DISCONNECTED";

/// The `ConnectionStateType` of a custom key store connected to its backing key
/// store: "You can create and use KMS keys in your custom key stores only when its
/// `ConnectionState` is `CONNECTED`" (the same member's documentation).
pub const CONNECTION_STATE_CONNECTED: &str = "CONNECTED";

/// The `ConnectionStateType` of a custom key store whose connection attempt did not
/// succeed: "A value of `FAILED` indicates that an attempt to connect was
/// unsuccessful. The `ConnectionErrorCode` field in the response indicates the cause
/// of the failure" (the same member's documentation).
pub const CONNECTION_STATE_FAILED: &str = "FAILED";

/// The `ConnectionStateType` of a custom key store whose connection process is under
/// way: connecting one "starts the connection process, but it does not wait for it to
/// complete", and "this response does not indicate that the custom key store is
/// connected" (`smithy_query(query="doc",shape="ConnectCustomKeyStore")`), so this is
/// the state the key store is left in until the connection falls due.
pub const CONNECTION_STATE_CONNECTING: &str = "CONNECTING";

/// The `ConnectionStateType` of a custom key store whose disconnection is under way.
/// "When you disconnect a custom key store, its `ConnectionState` changes to
/// `Disconnected`" (`smithy_query(query="doc",shape="DisconnectCustomKeyStore")`),
/// which it reaches through this transient state.
pub const CONNECTION_STATE_DISCONNECTING: &str = "DISCONNECTING";

/// How long a custom key store stays in the transient connection state a connect or
/// disconnect left it in before the change that call started falls due.
///
/// "The connection process for a custom key store can take an extended amount of time
/// to complete" (`smithy_query(query="doc",shape="ConnectCustomKeyStore")`) and the
/// model names no duration, so the span is this emulator's: short, but never zero, so
/// a caller that reads the key store straight after the call still observes the
/// transient state and the refusals that go with it.
pub const CUSTOM_KEY_STORE_CONNECTION_SPAN: Duration = Duration::from_millis(500);

/// The `CustomKeyStoreType` of a key store backed by a CloudHSM cluster, from
/// `smithy_query(shape="CustomKeyStoreType")`.
pub const CUSTOM_KEY_STORE_TYPE_AWS_CLOUDHSM: &str = "AWS_CLOUDHSM";

/// The `CustomKeyStoreType` of a key store backed by an external key store proxy
/// and an external key manager. See [`CUSTOM_KEY_STORE_TYPE_AWS_CLOUDHSM`].
pub const CUSTOM_KEY_STORE_TYPE_EXTERNAL_KEY_STORE: &str = "EXTERNAL_KEY_STORE";

/// The `XksProxyConnectivityType` of an external key store proxy reached through an
/// Amazon VPC endpoint service, from
/// `smithy_query(shape="XksProxyConnectivityType")`.
pub const XKS_PROXY_CONNECTIVITY_VPC_ENDPOINT_SERVICE: &str = "VPC_ENDPOINT_SERVICE";

/// What every custom key store id starts with, as AWS's own ids do.
const CUSTOM_KEY_STORE_ID_PREFIX: &str = "cks-";

/// A freshly minted custom key store id, drawn from the OS CSPRNG so two key
/// stores never share one.
///
/// `CustomKeyStoreIdType` is only "a unique identifier for the custom key store"
/// (`smithy_query(shape="CustomKeyStoresListEntry")`) with a modelled 1..64 length
/// and no pattern, so the form is this emulator's: the `cks-` prefix followed by 16
/// lowercase hex characters.
///
/// `None` means the platform refused to produce entropy; the caller answers a
/// modelled server error rather than minting a predictable id.
pub fn mint_custom_key_store_id() -> Option<String> {
    let mut bytes = [0u8; 8];
    SystemRandom::new().fill(&mut bytes).ok()?;
    Some(format!(
        "{CUSTOM_KEY_STORE_ID_PREFIX}{}",
        hex::encode(bytes)
    ))
}

/// One custom key store: a key store the customer owns and manages, which KMS
/// reports and connects to but holds no key material of.
#[derive(Debug, Clone)]
pub struct CustomKeyStore {
    /// The minted key store id, which is also this record's map key.
    pub custom_key_store_id: String,
    /// "The user-specified friendly name for the custom key store"
    /// (`smithy_query(shape="CustomKeyStoresListEntry")`), unique per account and
    /// Region.
    pub custom_key_store_name: String,
    /// The Region whose endpoint created it; a custom key store is a regional
    /// resource.
    pub region: String,
    pub creation_date: SystemTime,
    /// A `ConnectionStateType` value: whether the key store is connected to its
    /// backing key store.
    pub connection_state: String,
    /// The `ConnectionErrorCodeType` of the last failed connection attempt, which
    /// is reported only while [`Self::connection_state`] is
    /// [`CONNECTION_STATE_FAILED`].
    pub connection_error_code: Option<String>,
    /// What backs the key store, which is also what its `CustomKeyStoreType` is:
    /// the two are one field, so a key store can never report a type that
    /// disagrees with the configuration it holds.
    pub backing: CustomKeyStoreBacking,
    /// The connection change a connect or disconnect started and the clock still
    /// owes, if any — see [`CustomKeyStore::settle`].
    pub pending_connection: Option<PendingConnectionChange>,
}

impl CustomKeyStore {
    /// A freshly created custom key store, which "has never been connected" and so
    /// starts [`CONNECTION_STATE_DISCONNECTED`].
    pub fn new(
        custom_key_store_id: String,
        region: String,
        custom_key_store_name: String,
        backing: CustomKeyStoreBacking,
    ) -> Self {
        Self {
            custom_key_store_id,
            custom_key_store_name,
            region,
            creation_date: SystemTime::now(),
            connection_state: CONNECTION_STATE_DISCONNECTED.to_string(),
            connection_error_code: None,
            backing,
            pending_connection: None,
        }
    }

    /// Start the connection process: the key store is left
    /// [`CONNECTION_STATE_CONNECTING`] and owes [`CONNECTION_STATE_CONNECTED`] one
    /// [`CUSTOM_KEY_STORE_CONNECTION_SPAN`] later, because connecting one "starts
    /// the connection process, but it does not wait for it to complete"
    /// (`smithy_query(query="doc",shape="ConnectCustomKeyStore")`).
    pub fn start_connecting(&mut self) {
        self.begin(CONNECTION_STATE_CONNECTING, CONNECTION_STATE_CONNECTED);
    }

    /// Start the disconnection: the key store is left
    /// [`CONNECTION_STATE_DISCONNECTING`] and owes
    /// [`CONNECTION_STATE_DISCONNECTED`], the state "its `ConnectionState` changes
    /// to" when it is disconnected
    /// (`smithy_query(query="doc",shape="DisconnectCustomKeyStore")`).
    pub fn start_disconnecting(&mut self) {
        self.begin(
            CONNECTION_STATE_DISCONNECTING,
            CONNECTION_STATE_DISCONNECTED,
        );
    }

    /// The one place a connection change is started: the key store enters the
    /// transient state at once and the settled one is recorded as due, so no caller
    /// has to wait and no two callers disagree about when it arrives.
    fn begin(&mut self, transient: &str, settled: &str) {
        self.connection_state = transient.to_string();
        self.pending_connection = Some(PendingConnectionChange {
            connection_state: settled.to_string(),
            due_at: SystemTime::now()
                .checked_add(CUSTOM_KEY_STORE_CONNECTION_SPAN)
                .unwrap_or_else(SystemTime::now),
        });
    }

    /// Materialise the connection change the clock has ALREADY made due, so every
    /// later caller sees the state the key store is owed rather than the transient
    /// one the call that started it left behind.
    ///
    /// Nothing happens before the due instant, and nothing happens at all for a key
    /// store with no change outstanding, so this is idempotent and never moves a
    /// state because something was read.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(pending) = &self.pending_connection else {
            return;
        };
        if now < pending.due_at {
            return;
        }
        self.connection_state = pending.connection_state.clone();
        self.pending_connection = None;
    }

    /// This key store's `CustomKeyStoreType`, which follows from what backs it:
    /// "`AWS_CLOUDHSM` indicates a custom key store backed by an CloudHSM cluster.
    /// `EXTERNAL_KEY_STORE` indicates a custom key store backed by an external key
    /// store proxy and external key manager"
    /// (`smithy_query(query="doc",shape="CustomKeyStoresListEntry",member="CustomKeyStoreType")`).
    pub fn custom_key_store_type(&self) -> &'static str {
        match self.backing {
            CustomKeyStoreBacking::CloudHsm(_) => CUSTOM_KEY_STORE_TYPE_AWS_CLOUDHSM,
            CustomKeyStoreBacking::External(_) => CUSTOM_KEY_STORE_TYPE_EXTERNAL_KEY_STORE,
        }
    }

    /// This custom key store as `DescribeCustomKeyStores` reports it — the one place
    /// a key store is projected onto the wire, so two readers cannot disagree.
    ///
    /// A member a key store of this type has no value for is ABSENT rather than
    /// empty: the CloudHSM cluster and its trust anchor "appear only when the
    /// `CustomKeyStoreType` is `AWS_CLOUDHSM`", the proxy configuration "only when
    /// the `CustomKeyStoreType` is `EXTERNAL_KEY_STORE`", and the connection error
    /// code "only when the `ConnectionState` is `FAILED`"
    /// (`smithy_query(shape="CustomKeyStoresListEntry")`).
    pub fn list_entry(&self) -> CustomKeyStoresListEntry {
        let cloud_hsm = match &self.backing {
            CustomKeyStoreBacking::CloudHsm(cluster) => Some(cluster),
            CustomKeyStoreBacking::External(_) => None,
        };
        let proxy = match &self.backing {
            CustomKeyStoreBacking::External(proxy) => Some(proxy),
            CustomKeyStoreBacking::CloudHsm(_) => None,
        };
        CustomKeyStoresListEntry {
            custom_key_store_id: Some(self.custom_key_store_id.clone()),
            custom_key_store_name: Some(self.custom_key_store_name.clone()),
            cloud_hsm_cluster_id: cloud_hsm.map(|cluster| cluster.cloud_hsm_cluster_id.clone()),
            trust_anchor_certificate: cloud_hsm
                .map(|cluster| cluster.trust_anchor_certificate.clone()),
            connection_state: Some(self.connection_state.clone()),
            connection_error_code: self
                .connection_error_code
                .clone()
                .filter(|_| self.connection_state == CONNECTION_STATE_FAILED),
            creation_date: Some(date_time(self.creation_date)),
            custom_key_store_type: Some(self.custom_key_store_type().to_string()),
            xks_proxy_configuration: proxy.map(XksProxy::configuration),
        }
    }
}

/// A connection change a connect or disconnect started and the clock owes: the
/// `ConnectionStateType` the custom key store reaches, and the instant it falls due.
#[derive(Debug, Clone)]
pub struct PendingConnectionChange {
    /// The `ConnectionStateType` the key store takes on at [`Self::due_at`].
    pub connection_state: String,
    /// When the change falls due. It is stored rather than waited for, so no
    /// request sleeps and every reader agrees on when it arrives.
    pub due_at: SystemTime,
}

/// What backs a custom key store, and so what it reports about itself.
#[derive(Debug, Clone)]
pub enum CustomKeyStoreBacking {
    /// An CloudHSM cluster the customer owns.
    CloudHsm(CloudHsmCluster),
    /// An external key store proxy in front of an external key manager.
    External(XksProxy),
}

/// The CloudHSM cluster behind an `AWS_CLOUDHSM` custom key store.
#[derive(Debug, Clone)]
pub struct CloudHsmCluster {
    /// "A unique identifier for the CloudHSM cluster that is associated with an
    /// CloudHSM key store" (`smithy_query(shape="CustomKeyStoresListEntry")`).
    pub cloud_hsm_cluster_id: String,
    /// "The trust anchor certificate of the CloudHSM cluster associated with an
    /// CloudHSM key store" (the same shape).
    pub trust_anchor_certificate: String,
}

/// The external key store proxy behind an `EXTERNAL_KEY_STORE` custom key store.
///
/// `Debug` is written by hand rather than derived so the proxy's access key id is
/// REDACTED: the model marks it sensitive
/// (`smithy_query(shape="XksProxyConfigurationType")`), and a derived one would
/// print it into any log line or panic message that formats a key store record.
#[derive(Clone)]
pub struct XksProxy {
    /// An `XksProxyConnectivityType` value: "whether the external key store proxy
    /// uses a public endpoint or an Amazon VPC endpoint service to communicate with
    /// KMS" (`smithy_query(shape="XksProxyConfigurationType")`).
    pub connectivity: String,
    /// The access key id half of the proxy's authentication credential, which is
    /// the half `DescribeCustomKeyStores` reports.
    pub access_key_id: String,
    pub uri_endpoint: String,
    pub uri_path: String,
    /// The VPC endpoint service the proxy is reached through, and the account that
    /// owns it — set only for [`XKS_PROXY_CONNECTIVITY_VPC_ENDPOINT_SERVICE`].
    pub vpc_endpoint_service_name: Option<String>,
    pub vpc_endpoint_service_owner: Option<String>,
}

impl std::fmt::Debug for XksProxy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XksProxy")
            .field("connectivity", &self.connectivity)
            .field("access_key_id", &"<redacted>")
            .field("uri_endpoint", &self.uri_endpoint)
            .field("uri_path", &self.uri_path)
            .field("vpc_endpoint_service_name", &self.vpc_endpoint_service_name)
            .field(
                "vpc_endpoint_service_owner",
                &self.vpc_endpoint_service_owner,
            )
            .finish()
    }
}

impl XksProxy {
    /// This proxy as `DescribeCustomKeyStores` reports it. The VPC endpoint service
    /// members "appear only when the external key store proxy uses an Amazon VPC
    /// endpoint service" (`smithy_query(shape="XksProxyConfigurationType")`), so a
    /// proxy on a public endpoint reports neither.
    pub fn configuration(&self) -> XksProxyConfigurationType {
        let through_vpc_endpoint_service =
            self.connectivity == XKS_PROXY_CONNECTIVITY_VPC_ENDPOINT_SERVICE;
        XksProxyConfigurationType {
            connectivity: Some(self.connectivity.clone()),
            access_key_id: Some(self.access_key_id.clone()),
            uri_endpoint: Some(self.uri_endpoint.clone()),
            uri_path: Some(self.uri_path.clone()),
            vpc_endpoint_service_name: self
                .vpc_endpoint_service_name
                .clone()
                .filter(|_| through_vpc_endpoint_service),
            vpc_endpoint_service_owner: self
                .vpc_endpoint_service_owner
                .clone()
                .filter(|_| through_vpc_endpoint_service),
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
    /// When this key stops being in the transient [`KEY_STATE_CREATING`] state, or
    /// `None` for a key that was never in it. Only a replica key starts there, and
    /// only until [`REPLICA_CREATING_SPAN`] after the call that created it; a reader
    /// derives the transient state from this instant ([`Self::is_creating`]) rather
    /// than from anything a later call wrote.
    pub creating_until: Option<SystemTime>,
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
    /// The key policy document this key holds, in the layout KMS reports it in
    /// ([`attached_key_policy`]) — the document a request attached, or the one
    /// [`default_key_policy`] builds for a key created without one.
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
    /// The last successful cryptographic operation performed with this key, or
    /// `None` while it has had none — the record `GetKeyLastUsage` reports once the
    /// usage tracking has caught up with it ([`USAGE_TRACKING_LAG`]). Every
    /// cryptographic operation that succeeds writes it ([`Self::record_last_usage`]);
    /// a reporting operation never does.
    pub last_usage: Option<KeyUsage>,
    /// The newest usage record the tracking had ALREADY produced when [`Self::last_usage`]
    /// was written, or `None` while it had produced none.
    ///
    /// It is what `GetKeyLastUsage` goes on reporting while the newer use is still in the
    /// pipeline: a record the tracking has produced is replaced by a newer one, never
    /// erased by a use the tracking has not caught up with — see
    /// [`Self::reported_last_usage`].
    pub tracked_last_usage: Option<KeyUsage>,
    /// What the caller stated about the key material it imported, or `None` for a key
    /// whose material KMS generated and for one still waiting for material — see
    /// [`ImportedKeyMaterial`]. It is what the `ExpirationModel` and `ValidTo`
    /// metadata members report, and what makes the material expire.
    pub imported_material: Option<ImportedKeyMaterial>,
    /// Key material a caller imported for an on-demand rotation that has not happened
    /// yet, or `None` while none is waiting — see [`PendingRotationMaterial`].
    pub pending_rotation_material: Option<PendingRotationMaterial>,
    /// The on-demand rotation this key has STARTED whose completion the clock still
    /// owes, or `None` when none is in progress — see [`OnDemandRotation`].
    ///
    /// `RotateKeyOnDemand` sets it and every reader derives the rotation's completion
    /// from the instant it is due ([`ON_DEMAND_ROTATION_SPAN`]) rather than from a write,
    /// so no reader has to wait for one and two readers cannot disagree about when it
    /// landed.
    pub on_demand_rotation: Option<OnDemandRotation>,
}

/// An on-demand rotation of a KMS key's key material that has been initiated and is
/// still IN PROGRESS: "KMS uses a background process to perform rotations. As a result,
/// there might be a slight delay between initiating on-demand key rotation and the
/// rotation's completion"
/// (`smithy_query(query="doc",shape="GetKeyRotationStatusResponse",member="OnDemandRotationStartDate")`),
/// so the material it installs becomes current, and the rotation becomes one the key has
/// had, only once the rotation completes — [`ON_DEMAND_ROTATION_SPAN`] after the call that
/// started it. "You can use `GetKeyRotationStatus` to identify any in progress on-demand
/// rotations. You can use `ListKeyRotations` to identify the date that completed on-demand
/// rotations were performed" (`smithy_query(query="doc",shape="RotateKeyOnDemand")`) — the
/// two readers report the two sides of this record.
///
/// `Debug` is written by hand rather than derived so the material is REDACTED.
#[derive(Clone)]
pub struct OnDemandRotation {
    /// The material that becomes current when the rotation completes.
    pub material: KeyMaterial,
    /// When the rotation was initiated, which is what `GetKeyRotationStatus` reports as
    /// its `OnDemandRotationStartDate` and the date the completed rotation is recorded
    /// with, so the two cannot disagree.
    pub started_at: SystemTime,
    /// What the caller stated about the material, when it is material the caller
    /// IMPORTED, and `None` when KMS generated it.
    pub imported: Option<ImportedKeyMaterial>,
}

impl OnDemandRotation {
    /// When this rotation completes.
    pub fn completes_at(&self) -> SystemTime {
        self.started_at + ON_DEMAND_ROTATION_SPAN
    }

    /// Whether the clock has completed this rotation by `now`.
    pub fn is_complete(&self, now: SystemTime) -> bool {
        now >= self.completes_at()
    }
}

impl std::fmt::Debug for OnDemandRotation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OnDemandRotation")
            // The material itself never appears; its id identifies it without
            // disclosing it.
            .field("key_material_id", &self.material.id())
            .field("started_at", &self.started_at)
            .field("imported", &self.imported)
            .finish()
    }
}

/// Key material a caller has imported for an on-demand rotation of a KMS key, before
/// that rotation completes.
///
/// "`PENDING_ROTATION` identifies key material that has been imported for on-demand key
/// rotation but the rotation hasn't completed"
/// (`smithy_query(query="doc",shape="RotationsListEntry",member="KeyMaterialState")`), so
/// it is NOT the material the key encrypts under: it is held beside that material until
/// `RotateKeyOnDemand` makes it current. "Key material in `PENDING_ROTATION` state is not
/// permanently associated with the KMS key. You can delete this key material and import
/// different key material in its place" (same doc), which is why a later import replaces
/// it rather than being refused.
///
/// `Debug` is written by hand rather than derived so the material is REDACTED.
#[derive(Clone)]
pub struct PendingRotationMaterial {
    /// The material itself, which becomes the key's current material when the rotation
    /// completes.
    pub material: KeyMaterial,
    /// What the caller stated about it at the import that supplied it.
    pub imported: ImportedKeyMaterial,
}

impl std::fmt::Debug for PendingRotationMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingRotationMaterial")
            // The material itself never appears; its id identifies it without
            // disclosing it.
            .field("key_material_id", &self.material.id())
            .field("imported", &self.imported)
            .finish()
    }
}

/// One use of a KMS key in a successful cryptographic operation, as KMS's usage
/// tracking records it and `GetKeyLastUsage` reports it
/// (`smithy_query(query="shape",shape="KeyLastUsageData")`).
///
/// The record is written by the call it describes and reported only once the tracking
/// that produces it has caught up ([`USAGE_TRACKING_LAG`]), which is what a reader sees
/// as "the KeyLastUsage response element is empty" straight after a cryptographic
/// operation.
#[derive(Debug, Clone)]
pub struct KeyUsage {
    /// The name of the KMS API operation that used the key.
    pub operation: String,
    /// When it used the key.
    pub at: SystemTime,
    /// "The CloudTrail event ID of the cryptographic operation": the identifier of
    /// the CloudTrail event the call was recorded in, minted per call.
    pub cloud_trail_event_id: String,
    /// The id KMS gave the request that used the key, minted per call.
    pub kms_request_id: String,
}

impl KeyUsage {
    /// Whether the usage tracking has produced this record by `now`, which is what
    /// makes it reportable: it is due [`USAGE_TRACKING_LAG`] after the call it records.
    pub fn tracked_by(&self, now: SystemTime) -> bool {
        now >= self.at + USAGE_TRACKING_LAG
    }

    /// This record as `GetKeyLastUsage` projects it onto the wire — the one place a
    /// usage record is serialized, so two readers cannot disagree.
    pub fn last_usage_data(&self) -> KeyLastUsageData {
        KeyLastUsageData {
            operation: Some(self.operation.clone()),
            timestamp: Some(date_time(self.at)),
            cloud_trail_event_id: Some(self.cloud_trail_event_id.clone()),
            kms_request_id: Some(self.kms_request_id.clone()),
        }
    }
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
            .field("creating_until", &self.creating_until)
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
            .field("last_usage", &self.last_usage)
            .field("tracked_last_usage", &self.tracked_last_usage)
            .field("imported_material", &self.imported_material)
            .field("pending_rotation_material", &self.pending_rotation_material)
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
            // A key `CreateKey` mints is never in the transient `Creating` state:
            // only a replica key is ([`Self::replica_in`]).
            creating_until: None,
            description,
            key_usage,
            key_spec,
            origin,
            key_manager: KEY_MANAGER_CUSTOMER.to_string(),
            multi_region_topology,
            policy: attached_key_policy(&policy),
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
            // A new KMS key has not been used in a cryptographic operation yet, so
            // "the KeyLastUsage response element is empty"
            // (`smithy_query(query="doc",shape="GetKeyLastUsage")`).
            last_usage: None,
            tracked_last_usage: None,
            // A key created with no key material has not been given any yet, and a key
            // whose material KMS generated never carries imported material.
            imported_material: None,
            // Nothing has been imported for a rotation of a key that has just been
            // created.
            pending_rotation_material: None,
            // A key that has just been created has had no rotation at all.
            on_demand_rotation: None,
        }
    }

    /// The replica key of this multi-Region primary key that `ReplicateKey` creates
    /// in `region`.
    ///
    /// "A primary key and its replica keys share properties that make them
    /// interoperable. They have the same key ID and key material. They also have the
    /// same key spec, key usage, key material origin, and automatic key rotation
    /// status. KMS automatically synchronizes these shared properties among related
    /// multi-Region keys. All other properties of a replica key can differ, including
    /// its key policy, tags, aliases, and key state"
    /// (`smithy_query(query="doc",shape="ReplicateKey")`): the shared properties are
    /// taken from THIS key and everything else from the request, so the replica is a
    /// key of the same set rather than a copy of the record.
    ///
    /// "If you replicate a multi-Region primary key with imported key material, the
    /// replica key is created with no key material. You must import the same key
    /// material that you imported into the primary key" (same doc) — so a key whose
    /// material came from outside KMS replicates WITHOUT material, and its replica
    /// waits in [`KEY_STATE_PENDING_IMPORT`] once it is created.
    pub fn replica_in(
        &self,
        region: String,
        description: String,
        policy: String,
        tags: BTreeMap<String, String>,
    ) -> Self {
        let awaiting_import = self.origin == ORIGIN_EXTERNAL;
        let material = if awaiting_import {
            KeyMaterial::AwaitingImport
        } else {
            self.material.clone()
        };
        // The material a rotation replaced is part of the shared material history,
        // so a ciphertext the set sealed before a rotation opens at the replica's
        // endpoint too.
        let superseded_material = if awaiting_import {
            Vec::new()
        } else {
            self.superseded_material.clone()
        };
        let arn = key_arn(&region, &self.key_id);
        // One topology describes the whole set, and the replica joins it: the
        // Region it lives in now holds a replica key of the set.
        let mut topology = self
            .multi_region_topology
            .clone()
            .unwrap_or_else(|| MultiRegionTopology::primary_in(self.region.clone()));
        topology.add_replica_region(region.clone());
        Self {
            key_id: self.key_id.clone(),
            arn,
            aws_account_id: self.aws_account_id.clone(),
            region,
            // The replica is a new key, created now: its creation date is its own,
            // not the primary key's.
            creation_date: SystemTime::now(),
            enabled: !awaiting_import,
            key_state: if awaiting_import {
                KEY_STATE_PENDING_IMPORT.to_string()
            } else {
                KEY_STATE_ENABLED.to_string()
            },
            creating_until: Some(SystemTime::now() + REPLICA_CREATING_SPAN),
            description,
            key_usage: self.key_usage.clone(),
            key_spec: self.key_spec.clone(),
            origin: self.origin.clone(),
            key_manager: self.key_manager.clone(),
            multi_region_topology: Some(topology),
            policy: attached_key_policy(&policy),
            deletion_date: None,
            pending_window_in_days: None,
            tags,
            current_key_material_id: material.id(),
            material,
            superseded_material,
            rotations: if awaiting_import {
                Vec::new()
            } else {
                self.rotations.clone()
            },
            // The automatic key rotation status is a shared property, so the replica
            // starts on the set's schedule.
            rotation_schedule: self.rotation_schedule.clone(),
            // The replica has not been used in a cryptographic operation yet; what
            // the primary key was used for is the primary key's record, because
            // "primary and replica keys track last usage independently"
            // (`smithy_query(query="doc",shape="GetKeyLastUsage")`).
            last_usage: None,
            tracked_last_usage: None,
            // A replica of a key with imported material holds none until the same
            // material is imported into it, and one of a key whose material KMS
            // generated never carries imported material.
            imported_material: None,
            // What is waiting for a rotation of the primary key is waiting there: the
            // same material has to be imported into this replica for the set to rotate.
            pending_rotation_material: None,
            // A rotation in progress on the primary key is that key's: the replica is
            // created from the material the set has now.
            on_demand_rotation: None,
        }
    }

    /// Record that `operation` just used this key in a successful cryptographic
    /// operation.
    ///
    /// "Returns usage information about the last successful cryptographic operation
    /// performed with a specified KMS key"
    /// (`smithy_query(query="doc",shape="GetKeyLastUsage")`), so what is reported is
    /// always the latest use the tracking has produced a record for, and only a call that
    /// succeeded writes one: a refused call did not use the key. The CloudTrail event id
    /// and the request id belong to the call that is being recorded, so each is minted
    /// here rather than derived from the request.
    pub fn record_last_usage(&mut self, operation: &str) {
        let now = SystemTime::now();
        // The record the tracking has already PRODUCED is kept beside the new one, so
        // the use being recorded replaces nothing a reader has been told until the
        // tracking produces it too: "if the KeyLastUsage response element is present,
        // the key has been used for a successful cryptographic operation since the
        // TrackingStartDate"
        // (`smithy_query(query="doc",shape="GetKeyLastUsage")`) is a monotone fact
        // about the key, so a further use never takes it back to the empty record.
        // A use the tracking had NOT produced yet is superseded by this one instead —
        // the pipeline coalesces the two, and only the later record is ever reported,
        // which keeps what one key holds bounded however often it is used.
        if let Some(produced) = self.last_usage.take().filter(|usage| usage.tracked_by(now)) {
            self.tracked_last_usage = Some(produced);
        }
        self.last_usage = Some(KeyUsage {
            operation: operation.to_string(),
            at: now,
            cloud_trail_event_id: ::uuid::Uuid::new_v4().to_string(),
            kms_request_id: ::uuid::Uuid::new_v4().to_string(),
        });
    }

    /// The usage record `GetKeyLastUsage` reports for this key at `now`.
    ///
    /// The record a use of the key wrote is reported only once the usage tracking has
    /// produced it ([`KeyUsage::tracked_by`]); until then what is reported is the record
    /// the tracking produced BEFORE it ([`Self::tracked_last_usage`]), because the
    /// element's presence is a monotone fact about the key — a later use replaces the
    /// reported record and never erases it. For a key no use of which the tracking has
    /// produced a record for yet — including one whose only use is still in the pipeline,
    /// and one that has had no cryptographic operation at all — what is reported is the
    /// EMPTY record, every member of which is "absent if the key has not been used since
    /// KMS began tracking"
    /// (`smithy_query(query="shape",shape="KeyLastUsageData")`). Reporting it writes
    /// nothing: whether the tracking has caught up follows from the instant the use was
    /// recorded at, so every reader agrees without one of them persisting the answer.
    pub fn reported_last_usage(&self, now: SystemTime) -> KeyLastUsageData {
        self.last_usage
            .as_ref()
            .filter(|usage| usage.tracked_by(now))
            .or(self.tracked_last_usage.as_ref())
            .map(KeyUsage::last_usage_data)
            .unwrap_or_default()
    }

    /// Attach `policy` to this key, replacing the document it held.
    ///
    /// The document is stored in the layout KMS reports it in
    /// ([`attached_key_policy`]), the same layout the constructors above store the
    /// document they are given in, so every path that attaches a key policy leaves the
    /// key one text and `GetKeyPolicy` has nothing to re-render.
    pub fn attach_policy(&mut self, policy: &str) {
        self.policy = attached_key_policy(policy);
    }

    /// When KMS began recording cryptographic activity for this key, which is "the
    /// date the KMS key was created, whichever is later"
    /// (`smithy_query(query="doc",shape="GetKeyLastUsageResponse",member="TrackingStartDate")`):
    /// this store starts tracking a key when it mints it, so the later of the two is
    /// always the key's creation date.
    pub fn tracking_start_date(&self) -> SystemTime {
        self.creation_date
    }

    /// The `KeyMaterialId` of the material this key was CREATED with, which is the
    /// oldest version it holds: the first version a rotation superseded, or the
    /// current one while no rotation has happened. `None` for material KMS gives no
    /// id — see [`KeyMaterial::id`].
    pub fn first_key_material_id(&self) -> Option<String> {
        match self.superseded_material.first() {
            Some(material) => material.id(),
            None => self.current_key_material_id.clone(),
        }
    }

    /// Give this key the key material a caller imported, with what the caller stated
    /// about it, and report that material's `KeyMaterialId`.
    ///
    /// "Without its key material, the KMS key is unusable"
    /// (`smithy_query(query="doc",shape="ImportKeyMaterialRequest",member="ValidTo")`),
    /// so a key that was waiting for material becomes usable once it has some: it
    /// leaves [`KEY_STATE_PENDING_IMPORT`] for [`KEY_STATE_ENABLED`]. A key that
    /// already had material keeps the key state it is in — a re-import does not
    /// re-enable a disabled key.
    pub fn import_key_material(
        &mut self,
        material: KeyMaterial,
        imported: ImportedKeyMaterial,
    ) -> Option<String> {
        self.current_key_material_id = material.id();
        self.material = material;
        self.imported_material = Some(imported);
        if self.key_state == KEY_STATE_PENDING_IMPORT {
            self.key_state = KEY_STATE_ENABLED.to_string();
            self.enabled = true;
        }
        self.current_key_material_id.clone()
    }

    /// Whether this key retains the key material `key_material_id` names — a version a
    /// rotation replaced, which is kept so ciphertext sealed under it still decrypts.
    pub fn retains_key_material(&self, key_material_id: &str) -> bool {
        self.superseded_material
            .iter()
            // The version a rotation the clock has completed replaced is retained too,
            // whether or not the record holds that rotation yet.
            .chain(
                self.completed_on_demand_rotation()
                    .map(|_| &self.material),
            )
            .any(|material| material.id().as_deref() == Some(key_material_id))
    }

    /// Whether `material` is key material this KMS key is ALREADY associated with: the
    /// version it encrypts under now, any version it still retains, or the version
    /// already waiting for a rotation.
    ///
    /// It is what tells material a caller presents as NEW from material the key already
    /// has, and it compares the MATERIAL — every version of every kind this store holds
    /// the bytes of — rather than the `KeyMaterialId` a reader reports, which a key of
    /// only one kind carries. Material whose bytes are not held here is not recognised as
    /// a version of anything.
    pub fn associates_key_material(&self, material: &KeyMaterial) -> bool {
        let Some(digest) = material.digest() else {
            return false;
        };
        ::std::iter::once(&self.material)
            .chain(
                self.pending_rotation_material
                    .as_ref()
                    .map(|pending| &pending.material),
            )
            .chain(
                self.on_demand_rotation
                    .as_ref()
                    .map(|rotation| &rotation.material),
            )
            .chain(self.superseded_material.iter())
            .any(|version| version.digest().as_deref() == Some(digest.as_str()))
    }

    /// Whether this key has key material at all: a key still waiting for its first
    /// material has none.
    pub fn has_key_material(&self) -> bool {
        !matches!(self.effective_material(), KeyMaterial::AwaitingImport)
    }

    /// The PUBLIC half of this key's key pair, as a DER-encoded X.509
    /// `SubjectPublicKeyInfo` — what a caller downloads for an asymmetric KMS key,
    /// whose private half "never leaves KMS unencrypted"
    /// (`smithy_query(query="doc",shape="GetPublicKey")`).
    ///
    /// It is COMPUTED from the private key this store holds ([`public_key_spki`]), so
    /// it is the public half of the key that signs, decrypts or agrees under this KMS
    /// key, and reading it changes nothing.
    ///
    /// `None` for a key whose material is not a key pair whose private half this store
    /// holds: a symmetric or HMAC key, a key still waiting for imported material, or a
    /// key pair of a spec no library reachable from here generates.
    pub fn public_key(&self) -> Option<Vec<u8>> {
        public_key_spki(&self.key_spec, self.material.private_key_pkcs8()?)
    }

    /// The `KeyMaterialId` of the material waiting for an on-demand rotation of this
    /// key, or `None` while none is waiting — see [`PendingRotationMaterial`].
    pub fn pending_rotation_key_material_id(&self) -> Option<String> {
        self.pending_rotation_material
            .as_ref()
            .and_then(|pending| pending.material.id())
    }

    /// Drop the material waiting for a rotation, reporting its `KeyMaterialId`.
    ///
    /// "Key material in `PENDING_ROTATION` state is not permanently associated with the
    /// KMS key. You can delete this key material and import different key material in its
    /// place"
    /// (`smithy_query(query="doc",shape="RotationsListEntry",member="KeyMaterialState")`),
    /// so dropping it leaves the material the key encrypts under — and the key's
    /// usability — exactly as they were.
    ///
    /// `None` means nothing was waiting.
    pub fn forget_pending_key_material(&mut self) -> Option<String> {
        let pending = self.pending_rotation_material.take()?;
        pending.material.id()
    }

    /// Hold `material` beside the material this key encrypts under, for the on-demand
    /// rotation that will make it current, and report its `KeyMaterialId`.
    ///
    /// "When you initiate on-demand key rotation on a symmetric encryption KMS key with
    /// imported key material, you must have already imported new key material and that
    /// key material's state should be `PENDING_ROTATION`"
    /// (`smithy_query(query="doc",shape="RotateKeyOnDemand")`), so an import of NEW
    /// material into a key that already has material changes nothing the key is used
    /// with: what the key seals under, its key state and what it reports as current all
    /// stay as they were until [`Self::rotate_to_pending_material`] completes the
    /// rotation. Material already waiting is replaced, because material in that state
    /// "is not permanently associated with the KMS key".
    pub fn stage_key_material(
        &mut self,
        material: KeyMaterial,
        imported: ImportedKeyMaterial,
    ) -> Option<String> {
        let key_material_id = material.id();
        self.pending_rotation_material = Some(PendingRotationMaterial { material, imported });
        key_material_id
    }

    /// Complete the rotation the material waiting on this key was imported for: that
    /// material becomes the current version once the rotation completes, the version it
    /// replaces is retained, and the rotation is recorded — exactly as
    /// [`Self::start_on_demand_rotation`] does for material KMS generated, except that
    /// the rotation also keeps what the caller stated about the material it installed,
    /// which is what a reader reports for an imported version.
    ///
    /// `false` means nothing was waiting, so no rotation happened and nothing changed.
    pub fn rotate_to_pending_material(&mut self) -> bool {
        let Some(pending) = self.pending_rotation_material.take() else {
            return false;
        };
        self.start_on_demand_rotation(pending.material, Some(pending.imported));
        true
    }

    /// Delete the key material this key encrypts under, leaving it with none, and
    /// report the `KeyMaterialId` of what was deleted.
    ///
    /// A key with no key material "is unusable" until material is imported again
    /// (`smithy_query(query="doc",shape="DeleteImportedKeyMaterial")`), so the key is
    /// left waiting in [`KEY_STATE_PENDING_IMPORT`] — except while its deletion is
    /// scheduled, since "when the specified KMS key is in the PendingDeletion state,
    /// this operation does not change the KMS key's state" (the same doc).
    ///
    /// `None` means there was no key material to delete, which is the state the call
    /// asks for rather than a failure.
    pub fn delete_current_key_material(&mut self) -> Option<String> {
        let deleted = self.current_key_material_id.take();
        self.material = KeyMaterial::AwaitingImport;
        self.imported_material = None;
        // A rotation replaces the material the key encrypts under, so with none left
        // there is nothing to rotate FROM: material that was waiting for that rotation
        // goes with it rather than staying to be promoted onto a later, unrelated import.
        self.pending_rotation_material = None;
        if self.key_state != KEY_STATE_PENDING_DELETION {
            self.key_state = KEY_STATE_PENDING_IMPORT.to_string();
            self.enabled = false;
        }
        deleted
    }

    /// Delete one version of this key's material that a rotation retained, reporting
    /// the `KeyMaterialId` of what was deleted.
    ///
    /// The material the key encrypts under is untouched, so the key stays usable; the
    /// rotation that made the deleted version current is kept, because it happened.
    pub fn forget_key_material(&mut self, key_material_id: &str) -> Option<String> {
        let held = self
            .superseded_material
            .iter()
            .position(|material| material.id().as_deref() == Some(key_material_id))?;
        self.superseded_material.remove(held);
        Some(key_material_id.to_string())
    }

    /// The Amazon Web Services managed key behind a predefined
    /// `alias/aws/<service-name>` alias, in `region`.
    ///
    /// An Amazon Web Services managed key is the symmetric encryption key one
    /// service encrypts a customer's resources with: KMS generates its material and
    /// manages it, and its `KeyManager` ([`KEY_MANAGER_AWS`]) is what distinguishes
    /// it from a key the customer created
    /// (`smithy_query(query="doc",shape="KeyMetadata",member="KeyManager")`).
    ///
    /// Two things follow from that manager and are set here rather than left to a
    /// caller: KMS "always rotates the key material of Amazon Web Services managed
    /// keys every year" (`smithy_query(query="doc",shape="EnableKeyRotation")`), so
    /// the key carries that rotation schedule from the instant it exists — and the
    /// same sentence is why [`check_rotation_configurable`] refuses to let a caller
    /// enable or disable it. No description is invented: the per-service wording KMS
    /// gives these keys is not in the model.
    pub fn aws_managed(key_id: String, region: String, material: KeyMaterial) -> Self {
        let mut key = Self::new(
            key_id,
            region,
            String::new(),
            KEY_USAGE_ENCRYPT_DECRYPT.to_string(),
            KEY_SPEC_SYMMETRIC_DEFAULT.to_string(),
            ORIGIN_AWS_KMS.to_string(),
            false,
            default_key_policy(),
            BTreeMap::new(),
            material,
        );
        key.key_manager = KEY_MANAGER_AWS.to_string();
        key.schedule_rotation(Some(DEFAULT_ROTATION_PERIOD_IN_DAYS));
        key
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
        self.rotate_material_at(material, rotation_type, SystemTime::now(), None);
    }

    /// [`Self::rotate_material`], with the instant the rotation was INITIATED and what
    /// the caller stated about the material, when the material is material the caller
    /// imported.
    ///
    /// A rotation that takes a span to complete ([`OnDemandRotation`]) is recorded with
    /// the instant it started, not the instant it landed, so the date a reader reports
    /// for it is the one `GetKeyRotationStatus` reported while it was in progress.
    fn rotate_material_at(
        &mut self,
        material: KeyMaterial,
        rotation_type: &str,
        at: SystemTime,
        imported: Option<ImportedKeyMaterial>,
    ) {
        let key_material_id = material.id();
        let superseded = std::mem::replace(&mut self.material, material);
        self.superseded_material.push(superseded);
        self.current_key_material_id = key_material_id.clone();
        self.rotations.push(KeyRotation {
            key_material_id,
            rotation_date: at,
            rotation_type: rotation_type.to_string(),
            imported: imported.clone(),
        });
        // What the caller stated about the material that is now current is what the
        // metadata readers report from here on; material KMS generated is not imported
        // material, so there is nothing stated about it.
        if imported.is_some() {
            self.imported_material = imported;
        }
    }

    /// Start an on-demand rotation onto `material`, which becomes current when the
    /// rotation completes ([`ON_DEMAND_ROTATION_SPAN`] later).
    ///
    /// `imported` is what the caller stated about the material, for a rotation onto
    /// material the caller supplied, and `None` for material KMS generated.
    ///
    /// A rotation initiated while an earlier one is still in progress COMPLETES that
    /// earlier one first, so the material it installed is retained rather than dropped
    /// ("KMS retains all key material") and both rotations count against the quota.
    pub fn start_on_demand_rotation(
        &mut self,
        material: KeyMaterial,
        imported: Option<ImportedKeyMaterial>,
    ) {
        self.complete_on_demand_rotation();
        self.on_demand_rotation = Some(OnDemandRotation {
            material,
            started_at: SystemTime::now(),
            imported,
        });
    }

    /// Materialise an on-demand rotation the CLOCK has already completed, so the record
    /// holds what every reader already reports.
    ///
    /// A write the clock owed at that instant whether or not anybody called: no reader
    /// needs it, because each one derives the completion from the rotation's own due
    /// instant.
    pub fn settle_on_demand_rotation(&mut self) {
        if self.completed_on_demand_rotation().is_some() {
            self.complete_on_demand_rotation();
        }
    }

    /// Apply the on-demand rotation in progress, whether or not the clock has made it
    /// due. `None` was in progress is a no-op.
    fn complete_on_demand_rotation(&mut self) {
        let Some(rotation) = self.on_demand_rotation.take() else {
            return;
        };
        self.rotate_material_at(
            rotation.material,
            ROTATION_TYPE_ON_DEMAND,
            rotation.started_at,
            rotation.imported,
        );
    }

    /// The on-demand rotation this key has started that the clock has already completed,
    /// but whose completion the record does not hold yet.
    fn completed_on_demand_rotation(&self) -> Option<&OnDemandRotation> {
        self.on_demand_rotation
            .as_ref()
            .filter(|rotation| rotation.is_complete(SystemTime::now()))
    }

    /// The key material this key encrypts under NOW: the material of a rotation the
    /// clock has completed, otherwise the material the record holds as current.
    ///
    /// Until a rotation completes the key goes on encrypting under the material it had,
    /// which is what "an in progress on-demand rotation" means.
    pub fn effective_material(&self) -> &KeyMaterial {
        match self.completed_on_demand_rotation() {
            Some(rotation) => &rotation.material,
            None => &self.material,
        }
    }

    /// The `KeyMaterialId` a reader reports as this key's current one, which is the id of
    /// [`Self::effective_material`].
    pub fn reported_key_material_id(&self) -> Option<String> {
        match self.completed_on_demand_rotation() {
            Some(rotation) => rotation.material.id(),
            None => self.current_key_material_id.clone(),
        }
    }

    /// Every version of this key's material a ciphertext may have been sealed under,
    /// newest first — what `Decrypt` tries in order.
    pub fn open_candidates(&self) -> Vec<&KeyMaterial> {
        let mut candidates = Vec::with_capacity(self.superseded_material.len() + 2);
        candidates.push(self.effective_material());
        if self.completed_on_demand_rotation().is_some() {
            // The version the completed rotation replaced, which is retained.
            candidates.push(&self.material);
        }
        candidates.extend(self.superseded_material.iter().rev());
        candidates
    }

    /// The rotations a reader reports for this key, oldest first: the ones the record
    /// holds, plus one the clock has completed but the record does not hold yet.
    pub fn reported_rotations(&self) -> Vec<KeyRotation> {
        let mut reported = self.rotations.clone();
        if let Some(rotation) = self.completed_on_demand_rotation() {
            reported.push(KeyRotation {
                key_material_id: rotation.material.id(),
                rotation_date: rotation.started_at,
                rotation_type: ROTATION_TYPE_ON_DEMAND.to_string(),
                imported: rotation.imported.clone(),
            });
        }
        reported
    }

    /// How many ON_DEMAND rotations this key has had, which is what
    /// [`MAX_ON_DEMAND_ROTATIONS`] bounds. An automatic rotation does not count
    /// against that quota: it is a quota on on-demand rotation alone.
    ///
    /// A rotation still in progress counts: it has been performed, so the quota is
    /// charged for it at once rather than when it lands.
    pub fn on_demand_rotation_count(&self) -> usize {
        self.rotations
            .iter()
            .filter(|rotation| rotation.rotation_type == ROTATION_TYPE_ON_DEMAND)
            .count()
            + usize::from(self.on_demand_rotation.is_some())
    }

    /// When the on-demand rotation that is IN PROGRESS was initiated, or `None` when
    /// none is: "identifies the date and time that an in progress on-demand rotation
    /// was initiated … once the on-demand rotation is complete, KMS removes this field
    /// from the response"
    /// (`smithy_query(query="doc",shape="GetKeyRotationStatusResponse",member="OnDemandRotationStartDate")`).
    ///
    /// The rotation completes on the CLOCK, at the same instant its material becomes the
    /// one the key encrypts under ([`Self::effective_material`]), so a caller that sees
    /// the material advance never sees a start date beside it.
    pub fn on_demand_rotation_in_progress(&self) -> Option<SystemTime> {
        self.on_demand_rotation
            .as_ref()
            .filter(|rotation| !rotation.is_complete(SystemTime::now()))
            .map(|rotation| rotation.started_at)
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
        self.rotation_schedule = Some(RotationSchedule::starting_now(period_in_days));
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

    /// Whether this key is an ASYMMETRIC KMS key: one whose key material is a key
    /// pair, so the material KMS holds is the PRIVATE half and the public half is
    /// published ([`Cmk::public_key`]). It is the partition that decides which
    /// cryptographic scheme a key performs — "both symmetric or both asymmetric or
    /// both HMAC"
    /// (`smithy_query(query="doc",shape="UpdateAliasRequest",member="TargetKeyId")`) —
    /// and it is read from the key's spec profile rather than from one spec's name.
    pub fn is_asymmetric(&self) -> bool {
        key_spec_profile(&self.key_spec)
            .is_some_and(|profile| matches!(profile.material, MaterialKind::KeyPair))
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
            // "Specifies whether the KMS key is enabled. When KeyState is Enabled
            // this value is true, otherwise it is false"
            // (`smithy_query(query="doc",shape="KeyMetadata",member="Enabled")`), so
            // it is read from the state a READER reports: a replica still being
            // created, and a key whose imported material the clock has taken away,
            // are not enabled.
            enabled: self.reported_key_state() == KEY_STATE_ENABLED,
            description: Some(self.description.clone()),
            key_usage: Some(self.key_usage.clone()),
            key_state: Some(self.reported_key_state().to_string()),
            deletion_date: self.deletion_date.map(date_time),
            // Only a key with imported key material expires, and only when the caller
            // asked for material that does: both members are read from what the caller
            // stated at the import (`ImportedKeyMaterial`).
            valid_to: self
                .imported_material
                .as_ref()
                .and_then(|imported| imported.valid_to)
                .map(date_time),
            origin: Some(self.origin.clone()),
            custom_key_store_id: None,
            cloud_hsm_cluster_id: None,
            expiration_model: self
                .imported_material
                .as_ref()
                .map(|imported| imported.expiration_model.clone()),
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
            current_key_material_id: self
                .reported_key_material_id()
                .filter(|_| !self.key_material_expired()),
        }
    }

    /// The algorithms this key supports under `key_usage`, or `None` when that is
    /// not the usage the key was created with — which is what makes `KeyMetadata`
    /// carry exactly one of the four per-usage algorithm lists, and `GetPublicKey`
    /// exactly one of its three.
    pub fn algorithms_under(&self, key_usage: &str) -> Option<Vec<String>> {
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

    /// Whether this key is a REPLICA key of a multi-Region key set: a member of a set
    /// whose primary key lives in another Region.
    ///
    /// Every key of a set shares the set's one primary-key designation, so which member
    /// this is follows from the Region it lives in rather than from a flag of its own.
    /// One function for every operation that treats a replica differently from the
    /// primary, so they cannot disagree about which key is which.
    pub fn is_multi_region_replica(&self) -> bool {
        self.multi_region_topology
            .as_ref()
            .is_some_and(|topology| topology.primary_region() != self.region)
    }

    /// Whether this key's imported key material has reached the instant the caller set
    /// it to expire at.
    ///
    /// "When the key material expires, KMS deletes the key material from the KMS key.
    /// Without its key material, the KMS key is unusable"
    /// (`smithy_query(query="doc",shape="ImportKeyMaterialRequest",member="ValidTo")`) —
    /// a deletion the CLOCK makes due, so it is read from the instant the caller set
    /// rather than written by whoever happens to look.
    pub fn key_material_expired(&self) -> bool {
        self.imported_material
            .as_ref()
            .and_then(|imported| imported.valid_to)
            .is_some_and(|valid_to| SystemTime::now() >= valid_to)
    }

    /// The `KeyState` a READER reports for this key, which is the one it carries unless
    /// the clock still owes it a transient state change: a replica key is `Creating`
    /// until [`Self::creating_until`] passes, material past its `ValidTo` is deleted,
    /// leaving a key with no key material waiting in [`KEY_STATE_PENDING_IMPORT`], and
    /// and a key its set is still moving the primary-key designation through is
    /// `Updating` until that move is due ([`Self::is_updating_primary_region`]).
    pub fn reported_key_state(&self) -> &str {
        if self.is_creating() {
            return KEY_STATE_CREATING;
        }
        if self.key_material_expired() {
            return KEY_STATE_PENDING_IMPORT;
        }
        // A key whose set is still moving its primary Region reports the transient
        // state of that move, and "the original key state is restored when the update
        // is complete" — so the state the record carries is left exactly as it was, and
        // the transient state stands in for the ENABLED state it suspends. A key
        // awaiting deletion, awaiting key material or disabled goes on reporting what
        // it is: the move changes none of those, and each refuses what it refused.
        if self.key_state == KEY_STATE_ENABLED && self.is_updating_primary_region() {
            return KEY_STATE_UPDATING;
        }
        &self.key_state
    }

    /// Whether this key is still in the transient [`KEY_STATE_CREATING`] state, which
    /// the clock ends by itself — see [`REPLICA_CREATING_SPAN`].
    pub fn is_creating(&self) -> bool {
        self.creating_until
            .is_some_and(|until| SystemTime::now() < until)
    }

    /// Whether the clock still owes this key's multi-Region key set the primary-Region
    /// move a call has started, which is the transient [`KEY_STATE_UPDATING`] state —
    /// see [`PRIMARY_REGION_UPDATE_SPAN`]. The clock ends it by itself.
    ///
    /// Only the two keys the designation moves BETWEEN are in it ("the old and new
    /// primary keys have a transient key state of Updating"), so it is read off the
    /// move the set has in flight and the Region this key lives in — the same record
    /// the reported designation comes from, so the state and the designation can never
    /// disagree about whether the move is over.
    pub fn is_updating_primary_region(&self) -> bool {
        self.multi_region_topology
            .as_ref()
            .is_some_and(|topology| topology.is_moving_through(&self.region))
    }

    /// Whether this key can be used in a cryptographic operation right now.
    ///
    /// A key whose set is still moving its primary Region can: "while the key state is
    /// Updating, you can use the keys in cryptographic operations"
    /// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`) — a move in flight
    /// refuses the MANAGEMENT operations of the set, not its cryptographic ones.
    pub fn is_usable(&self) -> bool {
        self.enabled
            && matches!(
                self.reported_key_state(),
                KEY_STATE_ENABLED | KEY_STATE_UPDATING
            )
    }

    /// Whether this key's key state is one of the two `EnableKey` and `DisableKey`
    /// move between. A key awaiting deletion or awaiting imported key material is in
    /// neither, so those operations refuse it rather than dropping the state it is
    /// actually in — and neither is a key whose set is still moving its primary
    /// Region, since while that move is in flight "you cannot ... perform certain
    /// management operations, such as enabling or disabling these keys"
    /// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`).
    pub fn is_enableable(&self) -> bool {
        !self.is_updating_primary_region()
            && (self.key_state == KEY_STATE_ENABLED || self.key_state == KEY_STATE_DISABLED)
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
    /// What the caller stated about the material this rotation made current, when that
    /// material is material the caller IMPORTED, and `None` when KMS generated it — see
    /// [`ImportedKeyMaterial`]. It is what the imported-material members of this
    /// rotation's listing entry report, which are "only present for symmetric encryption
    /// KMS keys with EXTERNAL origin".
    pub imported: Option<ImportedKeyMaterial>,
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
    /// The schedule a call made at this instant configures: the period the request
    /// named, or [`DEFAULT_ROTATION_PERIOD_IN_DAYS`] when it named none, measured from
    /// now.
    ///
    /// One constructor, so a schedule written onto one key and a schedule written onto
    /// every key of a multi-Region set are the same schedule.
    fn starting_now(period_in_days: Option<i32>) -> Self {
        Self {
            period_in_days: period_in_days.unwrap_or(DEFAULT_ROTATION_PERIOD_IN_DAYS),
            scheduled_from: SystemTime::now(),
        }
    }

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
    /// operation", carrying the `message` KMS words that refusal with.
    ///
    /// A key whose KIND cannot rotate is refused with the error code ALONE
    /// ([`unsupported_rotation_message`]); a key that is a member of a multi-Region
    /// set and is not the member the property belongs on is refused with the sentence
    /// [`multi_region_replica_message`] builds, which names the key and the operation
    /// so the caller knows which key to call instead.
    Unsupported(Option<String>),
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

/// How KMS words its refusal of a request aimed at a multi-Region REPLICA key for a
/// property only the set's PRIMARY key carries: the key's own ARN, that it is a
/// multi-region replica key, and the operation it is not a valid resource for.
///
/// The variable parts are the replica key's ARN, read from state, and the name of the
/// operation that refused — so every operation that a replica key is not valid for
/// words its refusal the same way and names itself. One function for the whole
/// service, so no two of them word it differently.
#[must_use]
pub fn multi_region_replica_message(key_arn: &str, operation: &str) -> String {
    format!("{key_arn} is a multi-region replica key which is not valid for {operation}.")
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
/// `operation` is the operation asking, which is what its refusal of a replica key of
/// a multi-Region set names.
///
/// One function for both operations, so they cannot judge the same key differently.
pub fn check_rotation_configurable(
    key: &Cmk,
    operation: &str,
) -> Result<(), RotationConfigRefusal> {
    // "You cannot enable or disable automatic rotation of Amazon Web Services
    // managed KMS keys. KMS always rotates the key material of Amazon Web Services
    // managed keys every year."
    if key.key_manager != KEY_MANAGER_CUSTOMER {
        return Err(RotationConfigRefusal::Unsupported(
            unsupported_rotation_message(),
        ));
    }

    // Which keys cannot rotate automatically follows from the key's own kind and the
    // origin of its material, never from one spec's name: an asymmetric key and an
    // HMAC key are not symmetric encryption keys, and a key whose material did not
    // come from KMS is a key with imported key material.
    if !key.is_symmetric_encryption() || key.origin != ORIGIN_AWS_KMS {
        return Err(RotationConfigRefusal::Unsupported(
            unsupported_rotation_message(),
        ));
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
    // not carry the schedule; the primary key of its set does. The refusal names the
    // replica and the operation, because WHICH key the caller must call instead is
    // the whole of what it got wrong.
    if key.is_multi_region_replica() {
        return Err(RotationConfigRefusal::Unsupported(Some(
            multi_region_replica_message(&key.arn, operation),
        )));
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
    /// The Region holding the set's primary key once every move of the designation
    /// the clock owes has taken effect: the Region a `MultiRegion` create minted it
    /// in, until `UpdatePrimaryRegion` moves the designation elsewhere. A reader asks
    /// [`Self::primary_region`] instead, which is the designation as it stands NOW.
    settled_primary_region: String,
    /// The Regions holding a replica key of the set once every move of the
    /// designation the clock owes has taken effect. Sorted, so a reader reports the
    /// replicas in a stable order across calls.
    settled_replica_regions: BTreeSet<String>,
    /// The move of the designation `UpdatePrimaryRegion` started and the clock has
    /// not yet made due, or `None` when the set has no move outstanding — see
    /// [`PendingPromotion`].
    promotion: Option<PendingPromotion>,
}

impl MultiRegionTopology {
    /// A freshly minted multi-Region primary key's topology: primary in the Region
    /// it was created in, with no replica anywhere until one is created in another
    /// Region.
    pub fn primary_in(region: String) -> Self {
        Self {
            settled_primary_region: region,
            settled_replica_regions: BTreeSet::new(),
            promotion: None,
        }
    }

    /// Record that `region` now holds a replica key of this set.
    pub fn add_replica_region(&mut self, region: String) {
        self.settled_replica_regions.insert(region);
    }

    /// Start moving the designation to the replica key in `region`, due at `due`,
    /// answering whether that Region held a replica to promote at all.
    ///
    /// "This operation changes the replica key in the specified Region to a primary
    /// key and changes the former primary key to a replica key"
    /// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`), so the two moves
    /// are one step: the set never has two primary keys, nor none. A Region holding
    /// no replica of the set — including the one that already holds the primary key
    /// — has nothing to promote and leaves the topology exactly as it was.
    ///
    /// The move takes effect at `due` rather than at once, because until then "the
    /// process of updating the primary Region might still be in progress" (same
    /// doc): the designation this call moves and the transient key state that move
    /// puts the two keys in are the SAME record, so neither is ever observable
    /// without the other.
    pub fn promote(&mut self, region: &str, due: SystemTime) -> bool {
        self.settle_due_promotion();
        if !self.settled_replica_regions.contains(region) {
            return false;
        }
        self.promotion = Some(PendingPromotion {
            region: region.to_string(),
            due,
        });
        true
    }

    /// Whether the clock still owes this set the move of its designation a call has
    /// started — the set is transiently busy, which is what refuses the management
    /// operations of its keys.
    pub fn promotion_in_flight(&self) -> bool {
        self.promotion
            .as_ref()
            .is_some_and(|promotion| SystemTime::now() < promotion.due)
    }

    /// Whether the key `region` holds is one of the two the move in flight goes
    /// between — the old primary key and the new one, which are the keys that carry
    /// the transient [`KEY_STATE_UPDATING`] state while it is in flight.
    pub fn is_moving_through(&self, region: &str) -> bool {
        self.promotion.as_ref().is_some_and(|promotion| {
            SystemTime::now() < promotion.due
                && (promotion.region == region || self.settled_primary_region == region)
        })
    }

    /// The Region holding this set's primary key NOW: the Region a move the clock has
    /// already made due promoted, and otherwise the one the designation rests in. A
    /// move still in flight has not happened yet, so it moves nothing a reader sees.
    pub fn primary_region(&self) -> &str {
        match self.due_promotion() {
            Some(promoted) => promoted,
            None => &self.settled_primary_region,
        }
    }

    /// The Regions holding a replica key of this set NOW — every Region of the set
    /// but the one [`Self::primary_region`] reports, in the sorted order the backing
    /// set keeps them in.
    fn replica_regions(&self) -> BTreeSet<&str> {
        match self.due_promotion() {
            Some(promoted) => self
                .settled_replica_regions
                .iter()
                .map(String::as_str)
                .filter(|region| *region != promoted)
                .chain(std::iter::once(self.settled_primary_region.as_str()))
                .collect(),
            None => self
                .settled_replica_regions
                .iter()
                .map(String::as_str)
                .collect(),
        }
    }

    /// The Region a move the clock has ALREADY made due promoted, which is the
    /// designation this topology has not written down yet.
    fn due_promotion(&self) -> Option<&str> {
        self.promotion
            .as_ref()
            .filter(|promotion| SystemTime::now() >= promotion.due)
            .map(|promotion| promotion.region.as_str())
    }

    /// Write a move the clock has already made due into the designation, so a move
    /// started after it moves the set on from where that one left it. What a reader
    /// sees is unchanged: this is the same designation [`Self::primary_region`]
    /// already reports.
    fn settle_due_promotion(&mut self) {
        let Some(promoted) = self.due_promotion().map(str::to_string) else {
            return;
        };
        self.settled_replica_regions.remove(&promoted);
        let former_primary = std::mem::replace(&mut self.settled_primary_region, promoted);
        self.settled_replica_regions.insert(former_primary);
        self.promotion = None;
    }

    /// This set as the `MultiRegionConfiguration` of the key `region`'s endpoint
    /// serves: that key is the set's primary exactly when the primary Region is its
    /// own, and `ReplicaKeys` names every Region holding a replica, each by the key
    /// ARN that Region's endpoint serves it under.
    pub fn configuration(&self, key_id: &str, region: &str) -> MultiRegionConfiguration {
        let primary_region = self.primary_region();
        let key_type = if primary_region == region {
            MULTI_REGION_KEY_TYPE_PRIMARY
        } else {
            MULTI_REGION_KEY_TYPE_REPLICA
        };
        MultiRegionConfiguration {
            multi_region_key_type: Some(key_type.to_string()),
            primary_key: Some(multi_region_member(key_id, primary_region)),
            replica_keys: Some(
                self.replica_regions()
                    .into_iter()
                    .map(|replica| multi_region_member(key_id, replica))
                    .collect(),
            ),
        }
    }
}

/// The move of one multi-Region key set's primary-key designation that an
/// `UpdatePrimaryRegion` call started and the clock has not yet made due.
///
/// "Even after this operation completes, the process of updating the primary Region
/// might still be in progress for a few more seconds. ... The old and new primary
/// keys have a transient key state of Updating. The original key state is restored
/// when the update is complete"
/// (`smithy_query(query="doc",shape="UpdatePrimaryRegion")`) — so ONE record carries
/// both halves of the move: which Region the designation is going to, and the instant
/// it arrives there, which is also the instant the transient state it put the two keys
/// in is over.
#[derive(Debug, Clone)]
struct PendingPromotion {
    /// The Region whose replica key the move makes this set's primary key.
    region: String,
    /// When the move is due — [`PRIMARY_REGION_UPDATE_SPAN`] after the call that
    /// started it. Nothing has to be written when it passes: a reader derives the
    /// designation and the key state from it.
    due: SystemTime,
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
    /// An asymmetric key pair, of the key pair the spec names (see
    /// [`generate_key_pair`]).
    KeyPair,
}

impl MaterialKind {
    /// How many bytes of key material a key of this kind holds, or `None` for a kind
    /// this service does not hold the bytes of (an asymmetric key pair).
    ///
    /// It is what makes IMPORTED key material either the material the key needs or
    /// material that "does not meet the requirements" of the key it is imported into.
    pub fn material_len(&self) -> Option<usize> {
        match self {
            Self::Aes256 => Some(MATERIAL_LEN),
            Self::Hmac(len) => Some(*len),
            Self::KeyPair => None,
        }
    }
}

/// The key material behind one KMS key.
///
/// `Clone` is what lets a multi-Region replica key hold the SAME material as the
/// primary key it was replicated from: "they have the same key ID and key material"
/// (`smithy_query(query="doc",shape="ReplicateKey")`), which is what makes a
/// ciphertext produced in one Region decryptable in another. The bytes never leave
/// this process either way.
#[derive(Clone)]
pub enum KeyMaterial {
    /// The 256-bit AES-GCM key of a symmetric encryption KMS key, from the OS
    /// CSPRNG. `Encrypt` and `Decrypt` seal and open under exactly these bytes.
    Aes256([u8; MATERIAL_LEN]),
    /// The key of an HMAC KMS key: as many bytes from the OS CSPRNG as the spec's
    /// name gives bits.
    Hmac(Vec<u8>),
    /// A key pair of a spec no library reachable from here generates the key pair of
    /// (see [`generate_key_pair`]).
    ///
    /// The KMS key itself is real: it is persisted, reported by `DescribeKey` and
    /// `ListKeys`, and moves through its key states like any other. What is absent
    /// is the private key, so every operation that would USE it — `Sign`, `Verify`,
    /// `GetPublicKey`, `DeriveSharedSecret`, and `Encrypt`/`Decrypt` for an
    /// asymmetric spec — answers the scaffold's 501 rather than a fabricated
    /// signature, public key or ciphertext.
    KeyPairNotGenerated,
    /// The PRIVATE key of an asymmetric KMS key, as a DER-encoded PKCS#8
    /// `PrivateKeyInfo` — the form the wrapped material of an asymmetric key takes,
    /// and the form every key pair in this service is encoded in. It is either the key
    /// pair this service generated for the key ([`generate_key_pair`]) or the one a
    /// caller imported into it, held as the caller supplied it after it was parsed as a
    /// private key of the key pair the key's `KeySpec` names (see
    /// [`imported_key_pair`]) — so an operation that uses it signs with the key's own
    /// private half rather than with bytes invented for the occasion.
    ///
    /// The PUBLIC half is not stored beside it: it is derived from these bytes on
    /// demand ([`public_key_spki`]), so the two halves cannot drift apart.
    ///
    /// The bytes never leave this process: the private half of a KMS key "never leaves
    /// KMS unencrypted" (`smithy_query(query="doc",shape="CreateKey")`).
    PrivateKeyPkcs8(Vec<u8>),
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
    pub fn id(&self) -> Option<String> {
        let Self::Aes256(material) = self else {
            return None;
        };
        Some(sha256_hex(&[b"kms-key-material-id\0", material]))
    }

    /// The AES-256-GCM key behind a symmetric encryption KMS key, or `None` for
    /// material of any other kind.
    pub fn aes256(&self) -> Option<&[u8; MATERIAL_LEN]> {
        match self {
            Self::Aes256(material) => Some(material),
            Self::Hmac(_)
            | Self::KeyPairNotGenerated
            | Self::PrivateKeyPkcs8(_)
            | Self::AwaitingImport => None,
        }
    }

    /// The DER-encoded PKCS#8 private key behind an asymmetric KMS key, or `None` for
    /// material of any other kind — including a key pair whose private half this
    /// service does not hold.
    pub fn private_key_pkcs8(&self) -> Option<&[u8]> {
        match self {
            Self::PrivateKeyPkcs8(material) => Some(material),
            Self::Aes256(_)
            | Self::Hmac(_)
            | Self::KeyPairNotGenerated
            | Self::AwaitingImport => None,
        }
    }

    /// A value that identifies this material to THIS store, for every kind whose bytes
    /// it holds — unlike [`Self::id`], the `KeyMaterialId` a reader reports, which only
    /// one kind of material carries.
    ///
    /// It is a digest rather than the bytes, so comparing two versions of a key's
    /// material never compares the material itself. `None` for material whose bytes are
    /// not held here, which is therefore never equal to anything.
    fn digest(&self) -> Option<String> {
        let (kind, bytes): (&[u8], &[u8]) = match self {
            Self::Aes256(material) => (b"aes256", material),
            Self::Hmac(material) => (b"hmac", material),
            Self::PrivateKeyPkcs8(material) => (b"pkcs8", material),
            Self::KeyPairNotGenerated | Self::AwaitingImport => return None,
        };
        Some(sha256_hex(&[b"kms-key-material\0", kind, b"\0", bytes]))
    }

    /// Key material of `kind` made of exactly these bytes — how material a caller
    /// IMPORTED becomes the key material of a key, as [`mint_material`] is for material
    /// KMS generates.
    ///
    /// `None` means the bytes are not material of that kind: the wrong length for it, or
    /// a kind this service does not hold the bytes of.
    pub fn from_bytes(kind: MaterialKind, bytes: &[u8]) -> Option<Self> {
        if bytes.len() != kind.material_len()? {
            return None;
        }
        match kind {
            MaterialKind::Aes256 => Some(Self::Aes256(bytes.try_into().ok()?)),
            MaterialKind::Hmac(_) => Some(Self::Hmac(bytes.to_vec())),
            MaterialKind::KeyPair => None,
        }
    }
}

/// The SHA-256 of `parts`, as lowercase hex: the one construction both material
/// identifiers are built with ([`KeyMaterial::id`] and [`KeyMaterial::digest`]), so the
/// two cannot drift apart in form. Each identifier supplies its own leading domain string,
/// which is what keeps them distinct values over the same material.
fn sha256_hex(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hex::encode(hasher.finalize())
}

/// Key material a caller IMPORTED into an ASYMMETRIC KMS key of `key_spec`: the private
/// key of the key pair that spec names, as a DER-encoded PKCS#8 `PrivateKeyInfo`.
///
/// The bytes are material of THIS key only when they really are a private key of the key
/// pair `key_spec` names, which is what parsing them with the library that implements
/// that algorithm decides — nothing is taken on trust, and a private key of another spec
/// does not parse as one of this spec. `None` means they are not such a private key, or
/// `key_spec` names no key pair at all.
pub fn imported_key_pair(key_spec: &str, pkcs8: &[u8]) -> Option<KeyMaterial> {
    let parses = match key_pair_kind(key_spec)? {
        KeyPairKind::Rsa(modulus_bits) => ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(pkcs8)
            .is_ok_and(|private_key| private_key.key_size_bits() == modulus_bits),
        KeyPairKind::Ecdsa(signing_algorithm) => {
            ::aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(signing_algorithm, pkcs8).is_ok()
        }
        KeyPairKind::Ed25519 => ::aws_lc_rs::signature::Ed25519KeyPair::from_pkcs8(pkcs8).is_ok(),
        KeyPairKind::Sm2 => {
            <::sm2::SecretKey as ::sm2::pkcs8::DecodePrivateKey>::from_pkcs8_der(pkcs8).is_ok()
        }
    };
    parses.then(|| KeyMaterial::PrivateKeyPkcs8(pkcs8.to_vec()))
}

/// Whether this service can tell a private key of the key pair `key_spec` names from
/// other bytes, which is what it has to do before material a caller wrapped can become
/// such a key's material. `false` for a key pair no library here parses the private key
/// of, which the caller answers as not implemented rather than refusing correct material
/// as incorrect.
pub fn parses_key_pair(key_spec: &str) -> bool {
    key_pair_kind(key_spec).is_some()
}

/// Which asymmetric key pair one `KeySpec` names: one variant per family, described by
/// the parameter that differs (the RSA modulus size, the elliptic curve) rather than by a
/// case per spec.
enum KeyPairKind {
    /// An RSA key pair with a modulus of this many bits.
    Rsa(usize),
    /// An elliptic curve key pair on the curve this ECDSA signing algorithm names.
    Ecdsa(&'static ::aws_lc_rs::signature::EcdsaSigningAlgorithm),
    /// An Ed25519 key pair.
    Ed25519,
    /// An SM2 key pair, which `aws-lc-rs` does not expose and the `sm2` crate does.
    Sm2,
}

/// The key pair a `KeySpec` names, or `None` for a spec that names no key pair or one
/// whose private key no library reachable from here parses (an ML-DSA key pair).
///
/// Every other value of `smithy_query(query="shape",shape="KeySpec")` whose material is a
/// key pair is here: an `RSA_<bits>` spec states its modulus size in its own name, so it
/// is read from the spec the way [`data_key_bytes`] reads `AES_<bits>`, and each curve
/// spec names its curve.
fn key_pair_kind(key_spec: &str) -> Option<KeyPairKind> {
    use ::aws_lc_rs::signature::{
        ECDSA_P256K1_SHA256_ASN1_SIGNING, ECDSA_P256_SHA256_ASN1_SIGNING,
        ECDSA_P384_SHA384_ASN1_SIGNING, ECDSA_P521_SHA512_ASN1_SIGNING,
    };

    let profile = key_spec_profile(key_spec)?;
    if !matches!(profile.material, MaterialKind::KeyPair) {
        return None;
    }
    if let Some(bits) = key_spec.strip_prefix("RSA_") {
        let bits: usize = bits.parse().ok()?;
        return (bits > 0 && bits.is_multiple_of(8)).then_some(KeyPairKind::Rsa(bits));
    }
    match key_spec {
        "ECC_NIST_P256" => Some(KeyPairKind::Ecdsa(&ECDSA_P256_SHA256_ASN1_SIGNING)),
        "ECC_NIST_P384" => Some(KeyPairKind::Ecdsa(&ECDSA_P384_SHA384_ASN1_SIGNING)),
        "ECC_NIST_P521" => Some(KeyPairKind::Ecdsa(&ECDSA_P521_SHA512_ASN1_SIGNING)),
        "ECC_SECG_P256K1" => Some(KeyPairKind::Ecdsa(&ECDSA_P256K1_SHA256_ASN1_SIGNING)),
        "ECC_NIST_EDWARDS25519" => Some(KeyPairKind::Ed25519),
        "SM2" => Some(KeyPairKind::Sm2),
        _ => None,
    }
}

/// How many times a scalar is drawn before SM2 key generation gives up. A uniform
/// 32-byte string is below the curve order with probability above 1 - 2^-32, so a
/// handful of attempts is already beyond any practical failure.
const SM2_SCALAR_ATTEMPTS: usize = 8;

/// The length of an SM2 private scalar: the curve order is 256 bits
/// (GM/T 0003-2012, the standard the `sm2` crate implements).
const SM2_SCALAR_LEN: usize = 32;

/// One freshly generated asymmetric key pair, in the two DER encodings every key pair
/// in this service is held and reported in.
pub struct GeneratedKeyPair {
    /// The public half, as a DER-encoded X.509 `SubjectPublicKeyInfo` (SPKI), which is
    /// the encoding `PublicKeyType` carries.
    pub public_key_spki: Vec<u8>,
    /// The private half, as a DER-encoded PKCS#8 `PrivateKeyInfo`, which is what a
    /// `KeyMaterial::PrivateKeyPkcs8` holds. It never leaves this process in the clear.
    pub private_key_pkcs8: Vec<u8>,
}

/// The private half never appears: the public half identifies the pair without
/// disclosing it.
impl std::fmt::Debug for GeneratedKeyPair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GeneratedKeyPair")
            .field("public_key_spki", &self.public_key_spki)
            .field("private_key_pkcs8", &"<redacted>")
            .finish()
    }
}

/// The key pair `key_spec` names, drawn from the OS CSPRNG by the library that
/// implements that algorithm.
///
/// ONE generator for every key pair this service produces — the key material of an
/// asymmetric KMS key and the data key pair a caller asks for — so the two cannot
/// disagree about what a spec names or how a pair is encoded.
///
/// `None` means `key_spec` names no key pair this service generates (see
/// [`key_pair_kind`]) or the library refused to produce or encode one. Generating one
/// is CPU work, hundreds of milliseconds for the largest modulus, so a caller runs it
/// off the async worker and with no lock held.
pub fn generate_key_pair(key_spec: &str) -> Option<GeneratedKeyPair> {
    use ::aws_lc_rs::encoding::AsDer;
    use ::aws_lc_rs::signature::KeyPair as _;
    use ::sm2::pkcs8::{EncodePrivateKey, EncodePublicKey};

    match key_pair_kind(key_spec)? {
        KeyPairKind::Rsa(modulus_bits) => {
            let private_key =
                ::aws_lc_rs::rsa::PrivateDecryptingKey::generate(rsa_key_size(modulus_bits)?)
                    .ok()?;
            Some(GeneratedKeyPair {
                public_key_spki: private_key.public_key().as_der().ok()?.as_ref().to_vec(),
                private_key_pkcs8: private_key.as_der().ok()?.as_ref().to_vec(),
            })
        }
        KeyPairKind::Ecdsa(signing_algorithm) => {
            let pair = ::aws_lc_rs::signature::EcdsaKeyPair::generate(signing_algorithm).ok()?;
            Some(GeneratedKeyPair {
                public_key_spki: pair.public_key().as_der().ok()?.as_ref().to_vec(),
                private_key_pkcs8: pair.to_pkcs8v1().ok()?.as_ref().to_vec(),
            })
        }
        KeyPairKind::Ed25519 => {
            let pair = ::aws_lc_rs::signature::Ed25519KeyPair::generate().ok()?;
            Some(GeneratedKeyPair {
                public_key_spki: pair.public_key().as_der().ok()?.as_ref().to_vec(),
                private_key_pkcs8: pair.to_pkcs8v1().ok()?.as_ref().to_vec(),
            })
        }
        // The `sm2` crate derives a public key from a scalar but draws no scalar of
        // its own here: the bytes come from the same OS CSPRNG every other key this
        // service generates is drawn from, and a draw that is not a valid scalar is
        // redrawn rather than reduced, so every scalar is uniform over the curve
        // order.
        KeyPairKind::Sm2 => (0..SM2_SCALAR_ATTEMPTS).find_map(|_| {
            let scalar = random_bytes(SM2_SCALAR_LEN)?;
            let secret = ::sm2::SecretKey::from_slice(&scalar).ok()?;
            Some(GeneratedKeyPair {
                public_key_spki: secret.public_key().to_public_key_der().ok()?.as_ref().to_vec(),
                private_key_pkcs8: secret.to_pkcs8_der().ok()?.as_bytes().to_vec(),
            })
        }),
    }
}

/// The PUBLIC half of the key pair `private_key_pkcs8` is the private half of, as a
/// DER-encoded X.509 `SubjectPublicKeyInfo`.
///
/// It is COMPUTED from the private key by the library that implements the algorithm
/// `key_spec` names, so the public half a caller downloads is the public half of the
/// private key this store holds — whether that key was generated here or imported.
///
/// `None` means the bytes are not a private key of the key pair `key_spec` names, or
/// the library refused to encode its public half.
pub fn public_key_spki(key_spec: &str, private_key_pkcs8: &[u8]) -> Option<Vec<u8>> {
    use ::aws_lc_rs::encoding::AsDer;
    use ::aws_lc_rs::signature::KeyPair as _;
    use ::sm2::pkcs8::EncodePublicKey;

    match key_pair_kind(key_spec)? {
        KeyPairKind::Rsa(modulus_bits) => {
            let private_key =
                ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(private_key_pkcs8).ok()?;
            if private_key.key_size_bits() != modulus_bits {
                return None;
            }
            Some(private_key.public_key().as_der().ok()?.as_ref().to_vec())
        }
        KeyPairKind::Ecdsa(signing_algorithm) => {
            let pair =
                ::aws_lc_rs::signature::EcdsaKeyPair::from_pkcs8(signing_algorithm, private_key_pkcs8)
                    .ok()?;
            Some(pair.public_key().as_der().ok()?.as_ref().to_vec())
        }
        KeyPairKind::Ed25519 => {
            let pair = ::aws_lc_rs::signature::Ed25519KeyPair::from_pkcs8(private_key_pkcs8).ok()?;
            Some(pair.public_key().as_der().ok()?.as_ref().to_vec())
        }
        KeyPairKind::Sm2 => {
            let secret =
                <::sm2::SecretKey as ::sm2::pkcs8::DecodePrivateKey>::from_pkcs8_der(
                    private_key_pkcs8,
                )
                .ok()?;
            Some(secret.public_key().to_public_key_der().ok()?.as_ref().to_vec())
        }
    }
}

/// The `aws-lc-rs` RSA key size for a modulus of `modulus_bits` bits, or `None` for a
/// modulus size that library does not generate. Every `RSA_<bits>` key spec KMS
/// declares is one of these.
fn rsa_key_size(modulus_bits: usize) -> Option<::aws_lc_rs::rsa::KeySize> {
    match modulus_bits {
        2048 => Some(::aws_lc_rs::rsa::KeySize::Rsa2048),
        3072 => Some(::aws_lc_rs::rsa::KeySize::Rsa3072),
        4096 => Some(::aws_lc_rs::rsa::KeySize::Rsa4096),
        _ => None,
    }
}

/// The key material a key of `profile`'s spec is minted with.
///
/// Symmetric and HMAC material is drawn from the OS CSPRNG; an asymmetric spec is
/// minted a key pair of that spec ([`generate_key_pair`]), of which the store keeps
/// the private half. A spec whose key pair no library reachable from here generates
/// yields [`KeyMaterial::KeyPairNotGenerated`]: the KMS key is still real, and the
/// operations that would use its private half say so rather than inventing bytes that
/// are not a key pair. Material a caller IMPORTS is [`imported_key_pair`].
///
/// `None` means the platform refused to produce entropy or a key pair; the caller
/// answers a modelled server error rather than minting a key with predictable
/// material.
pub fn mint_material(profile: &KeySpecProfile) -> Option<KeyMaterial> {
    match profile.material {
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
        MaterialKind::KeyPair if key_pair_kind(profile.key_spec).is_none() => {
            Some(KeyMaterial::KeyPairNotGenerated)
        }
        MaterialKind::KeyPair => Some(KeyMaterial::PrivateKeyPkcs8(
            generate_key_pair(profile.key_spec)?.private_key_pkcs8,
        )),
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
const RSA_ENCRYPTION_ALGORITHMS: &[&str] = &[
    ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_1,
    ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_256,
];

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
/// KMS gives one so a client can recognise it on sight:
/// [`MULTI_REGION_KEY_ID_PREFIX`] followed by 32 hexadecimal characters.
pub fn mint_key_id(multi_region: bool) -> String {
    let id = ::uuid::Uuid::new_v4();
    if multi_region {
        format!("{MULTI_REGION_KEY_ID_PREFIX}{}", id.simple())
    } else {
        id.to_string()
    }
}

/// What a multi-Region key id begins with, which the modelled examples of such an id
/// show: `mrk-1234abcd12ab34cd56ef1234567890ab`
/// (`smithy_query(query="doc",shape="ReplicateKeyRequest",member="KeyId")`). One constant
/// for the whole service, so the form that is MINTED is the form that is RECOGNISED
/// ([`names_a_key_id`]).
pub const MULTI_REGION_KEY_ID_PREFIX: &str = "mrk-";

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

/// How KMS refuses a `TagList` that names one tag key twice: entirely FIXED text, which
/// names neither the repeated key nor the position it repeats at.
///
/// The list as a whole is what the request got wrong — "you cannot have more than one tag
/// on a KMS key with the same tag key"
/// (`smithy_query(query="doc",shape="TagResourceRequest",member="Tags")`) — so a caller
/// matching the refusal text to tell a repeated key from the other `TagException` causes
/// reads a constant, and no request value travels in it.
///
/// One constant for every operation that accepts tags, reached through
/// [`requested_tags`].
pub const DUPLICATE_TAG_KEYS_MESSAGE: &str = "Duplicate tag keys";

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
/// What this function pins is WHICH tag lists are refused, and
/// [`DUPLICATE_TAG_KEYS_MESSAGE`] how the repeated-key half of that is worded.
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
            return Err(DUPLICATE_TAG_KEYS_MESSAGE.to_string());
        }
    }
    Ok(requested)
}

/// AWS's default key policy, which "gives the Amazon Web Services account that owns
/// the KMS key permission to use it in all cryptographic operations"
/// (`smithy_query(query="doc",shape="CreateKeyRequest",member="Policy")`). Used
/// when `CreateKey` is given no `Policy`.
///
/// The document is written out and then rendered ([`attached_key_policy`]) rather than
/// built as a parsed value: a value's members would be re-ordered by whatever map
/// `serde_json` was built with, and this document's elements are written in the order
/// the default policy declares them.
pub fn default_key_policy() -> String {
    // `arn:aws:iam::<account>:root` — IAM is global, so the Region segment of the
    // principal's ARN is empty. Written as a JSON string literal, so a principal
    // needing an escape carries one.
    let root = ::serde_json::Value::String(::engine::emulator_arn("iam", "", "root"));
    // The current policy language version, the first of the two the IAM policy
    // reference documents (`::service_common::iam_policy::POLICY_VERSIONS`).
    let version = ::service_common::iam_policy::POLICY_VERSIONS[0];
    attached_key_policy(&format!(
        "{{\"Version\":\"{version}\",\"Id\":\"{DEFAULT_KEY_POLICY_ID}\",\"Statement\":\
         [{{\"Sid\":\"{DEFAULT_KEY_POLICY_SID}\",\"Effect\":\"Allow\",\
         \"Principal\":{{\"AWS\":{root}}},\"Action\":\"kms:*\",\"Resource\":\"*\"}}]}}"
    ))
}

/// The `Id` of the default key policy, as AWS's own default document declares it.
const DEFAULT_KEY_POLICY_ID: &str = "key-default-1";

/// The `Sid` of the one statement of the default key policy, as AWS's own default
/// document declares it: the statement that gives the account that owns the key
/// permission to use it.
const DEFAULT_KEY_POLICY_SID: &str = "Enable IAM User Permissions";

/// A key policy document in the layout KMS REPORTS it in, which is the layout a key
/// holds it in: the canonical rendering of an IAM policy document
/// ([`::service_common::iam_policy::canonical_document`]), shared with every other
/// emulated service that attaches one.
///
/// KMS does not serve the caller's bytes back: it parses the document a request carries
/// and re-serializes it, so `GetKeyPolicy` answers the same text whatever layout the
/// document was submitted in, and a caller that re-attaches what it read reads the same
/// text again. Applied wherever a policy is attached — `CreateKey`, `PutKeyPolicy`,
/// `ReplicateKey` and the default document above — so no path stores a document in any
/// other layout, and every reader of [`Cmk::policy`] reports the one rendering.
///
/// Text that is not a document KMS would accept is rendered as it came; which documents
/// are acceptable is [`judge_key_policy`]'s reading, and it runs before this.
pub fn attached_key_policy(policy: &str) -> String {
    ::service_common::iam_policy::canonical_document(policy)
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

/// Whether `policy` is a key policy document KMS would accept.
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
/// ineffective" (same doc). The policy-lockout safety check is the OTHER half of
/// judging a document and is [`authorization::permits_policy_management`]; both are
/// reached through [`judge_key_policy`], which every operation that attaches a key
/// policy calls.
///
/// This answers WHICH documents are refused and nothing about why: the refusal carries
/// no message (see [`KeyPolicyRefusal`]), so there is no sentence to word here.
pub fn is_well_formed_key_policy(policy: &str) -> bool {
    let Ok(document) = ::serde_json::from_str::<::serde_json::Value>(policy) else {
        return false;
    };
    let Some(object) = document.as_object() else {
        return false;
    };
    // An IAM JSON policy document's `Statement` is either one statement or a list
    // of them; a key policy with no statement at all grants nothing and is not a
    // document KMS stores.
    let statements: Vec<&::serde_json::Value> = match object.get("Statement") {
        Some(::serde_json::Value::Array(statements)) if !statements.is_empty() => {
            statements.iter().collect()
        }
        Some(statement @ ::serde_json::Value::Object(_)) => vec![statement],
        Some(_) | None => return false,
    };
    statements.iter().all(|statement| {
        let Some(statement) = statement.as_object() else {
            return false;
        };
        matches!(
            statement.get("Effect").and_then(::serde_json::Value::as_str),
            Some("Allow" | "Deny")
        ) && names_a_principal(statement)
    })
}

/// How KMS words the refusal of a key policy that would lock the calling principal out
/// of managing the key: the safety check's own sentence, entirely FIXED text that names
/// neither the principal nor the key.
pub const POLICY_LOCKOUT_MESSAGE: &str =
    "The new key policy will not allow you to update the key policy in the future.";

/// Why KMS will not attach a key policy document a request carried. Both classes are
/// the modelled `MalformedPolicyDocumentException` ("the specified policy is not
/// syntactically or semantically correct") and differ only in the message they carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyPolicyRefusal {
    /// The document is not one KMS accepts ([`is_well_formed_key_policy`]).
    ///
    /// It carries NO message. Which criterion the document failed is not disclosed —
    /// and a parser's own account of it (a position in the request's bytes) is not
    /// something KMS tells a caller.
    Malformed,
    /// The document would leave the calling principal no path to a subsequent
    /// `PutKeyPolicy` on the key, and the request did not ask for the safety check to be
    /// bypassed: [`POLICY_LOCKOUT_MESSAGE`].
    Lockout,
}

impl KeyPolicyRefusal {
    /// The message this refusal carries on the wire, `None` where it carries none.
    #[must_use]
    pub fn message(self) -> Option<String> {
        match self {
            Self::Malformed => None,
            Self::Lockout => Some(POLICY_LOCKOUT_MESSAGE.to_string()),
        }
    }
}

/// Whether `policy` is a document this request may attach to the key `key_arn`, judged
/// for the principal that made the request.
///
/// Two criteria, in the order KMS states them on the `Policy` member
/// (`smithy_query(query="doc",shape="PutKeyPolicyRequest",member="Policy")`): the
/// document has to be a key policy document at all, and it "must allow the calling
/// principal to make a subsequent PutKeyPolicy request on the KMS key. This reduces the
/// risk that the KMS key becomes unmanageable. … (To omit this condition, set
/// BypassPolicyLockoutSafetyCheck to true.)" — so the second criterion is skipped for
/// exactly the request that asks for it, and is in force by default, the default the
/// member itself declares.
///
/// Nothing is written by judging, so a refused document leaves the key the policy it
/// had. One function for every operation that attaches a key policy, so `CreateKey`,
/// `PutKeyPolicy` and `ReplicateKey` cannot accept different documents.
pub fn judge_key_policy(
    policy: &str,
    caller: &authorization::Caller,
    key_arn: &str,
    bypass_policy_lockout_safety_check: bool,
) -> Result<(), KeyPolicyRefusal> {
    if !is_well_formed_key_policy(policy) {
        return Err(KeyPolicyRefusal::Malformed);
    }
    if !bypass_policy_lockout_safety_check
        && !authorization::permits_policy_management(caller, policy, key_arn)
    {
        return Err(KeyPolicyRefusal::Lockout);
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

/// How an identifier that names an ALIAS is refused by an operation whose `KeyId`
/// takes a key id or key ARN only.
///
/// The KIND of identifier is what is wrong with such a request, which KMS judges from
/// the identifier's grammar BEFORE it looks anything up — so the sentence is fixed and
/// names neither the alias nor whether this endpoint holds one. Ordering it before the
/// lookup is what keeps the refusal from reporting the existence of an alias to a
/// caller this operation would never serve one to.
///
/// One constant for the whole service, so no two operations word the same refusal
/// differently.
pub const ALIASES_NOT_SUPPORTED_MESSAGE: &str = "Key Aliases are not supported for this operation.";

/// How KMS refuses an operation on a key whose `KeyUsage` does not permit it: "the
/// KeyUsage value of the KMS key is incompatible with the API operation"
/// (`smithy_query(query="doc",shape="InvalidKeyUsageException")`).
///
/// The sentence names the key by its ARN, the usage the key carries and the operation
/// that refused it — the three facts a caller needs to see which of the two is wrong,
/// the key or the call — and nothing else.
///
/// One function for the whole service, so no two cryptographic operations word this
/// refusal differently.
pub fn wrong_key_usage_message(key: &Cmk, operation: &str) -> String {
    format!(
        "{} key usage is {} which is not valid for {operation}.",
        key.arn, key.key_usage
    )
}

/// How KMS refuses an algorithm the key's own key spec does not support: the
/// algorithm "specified for the operation is incompatible with the type of key
/// material in the KMS key (KeySpec)"
/// (`smithy_query(query="doc",shape="InvalidKeyUsageException")`).
///
/// The sentence names the algorithm the request asked for and the key spec it is
/// incompatible with — the pair that is in conflict — and not the key: a caller who
/// named the key already holds its identifier, and which algorithms a key spec
/// supports is what `DescribeKey` reports.
///
/// One function for the whole service, so an encryption algorithm, a signing
/// algorithm and a MAC algorithm are all refused in the same words.
pub fn incompatible_algorithm_message(algorithm: &str, key_spec: &str) -> String {
    format!("Algorithm {algorithm} is incompatible with key spec {key_spec}.")
}

/// How KMS refuses an encryption context an algorithm cannot bind: "an encryption context
/// is valid only for cryptographic operations with a symmetric encryption KMS key. The
/// standard asymmetric encryption algorithms and HMAC algorithms that KMS uses do not
/// support an encryption context"
/// (`smithy_query(query="doc",shape="EncryptRequest",member="EncryptionContext")`).
///
/// The sentence names the algorithm the request asked for, which is the half of the pair
/// the caller can change — the member it carried is the other half and is in the request.
///
/// One function for the whole service, beside [`wrong_key_usage_message`] and
/// [`incompatible_algorithm_message`], so the refusals an algorithm meets are worded in
/// one place.
pub fn unsupported_encryption_context_message(algorithm: &str) -> String {
    format!("Algorithm {algorithm} does not support an encryption context.")
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
    key_state_refusal_message(key, key.reported_key_state())
}

/// [`invalid_state_message`] for a key state the caller is being refused for that a
/// READER does not report on this key record — the transient state of a change in
/// flight across a multi-Region key SET, which the key the request named need not be
/// one of the keys carrying.
///
/// One wording for both, so a key state refuses every request the same way.
pub fn key_state_refusal_message(key: &Cmk, key_state: &str) -> String {
    format!("{} is {}.", key.arn, key_state_prose(key_state))
}

/// How KMS words the refusal of a request that REQUIRES a key state the key is not in
/// — the negative of [`invalid_state_message`]: where an operation refused because of
/// the state the key is in names that state, one refused because the key is outside
/// the single state it needs names the state it is missing, "<key arn> is not pending
/// deletion."
pub fn required_state_message(key: &Cmk, required_state: &str) -> String {
    format!("{} is not {}.", key.arn, key_state_prose(required_state))
}

/// How an operation that acts on a multi-Region key SET refuses a key that belongs
/// to none, naming the key the request resolved to by its ARN.
///
/// `ReplicateKey` replicates "a multi-Region primary key" and `UpdatePrimaryRegion`
/// "changes the primary key of a multi-Region key"
/// (`smithy_query(query="doc",shape="ReplicateKey")`), so for both of them a
/// single-Region key is a resource that is not valid for the operation — the modelled
/// `UnsupportedOperationException`. One function for both, so the two cannot word the
/// same refusal differently.
pub fn not_multi_region_message(key: &Cmk) -> String {
    format!("{} is not a multi-region key.", key.arn)
}

/// How an operation that acts on the PRIMARY key of a multi-Region key set refuses a
/// REPLICA key of one.
///
/// `ReplicateKey`'s `KeyId` "identifies the multi-Region primary key that is being
/// replicated"
/// (`smithy_query(query="doc",shape="ReplicateKeyRequest",member="KeyId")`), so a
/// replica key of a set is a resource that is not valid for it; the refusal names the
/// key the request resolved to and the Region that does hold the set's primary key,
/// which is where the call belongs.
pub fn not_primary_key_message(key: &Cmk, primary_region: &str) -> String {
    format!(
        "{} is not a multi-region primary key. The primary key of this multi-region key is in \
         {primary_region}.",
        key.arn
    )
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
    let cipher = cipher(key.effective_material())?;
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
    key.open_candidates()
        .into_iter()
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
// Asymmetric encryption: a ciphertext produced OUTSIDE KMS
// ---------------------------------------------------------------------------

/// Why a ciphertext presented for an asymmetric KMS key could not be opened with the
/// private half this store holds, or why a plaintext could not be sealed to its public
/// half.
#[derive(Debug)]
pub enum AsymmetricRefusal {
    /// No library reachable from here performs the encryption scheme this key pair and
    /// algorithm name, so there is no plaintext to answer with. The KMS key is still
    /// real — the caller is told the capability is not written yet rather than handed
    /// bytes that are not the plaintext of its ciphertext.
    Unsupported,
    /// The bytes are not a ciphertext this key pair's private half opens under those
    /// parameters: they were produced for another key, under another algorithm, or
    /// they have been altered.
    NotThisCiphertext,
    /// The library implements the scheme and refused this particular operation, with the
    /// request already judged against every rule the scheme states. Nothing about the
    /// request explains it, so it is a server fault rather than a refusal.
    Failed,
}

/// The plaintext of `ciphertext`, which was encrypted under the PUBLIC half of `key`'s
/// key pair — the bytes `GetPublicKey` publishes ([`public_key_spki`]) — with
/// `encryption_algorithm`.
///
/// This is the one-way path an asymmetric encryption KMS key exists for: a party
/// holding only the published public key encrypts without calling KMS at all, and only
/// the private half held here opens the result. Nothing about the ciphertext says which
/// key or algorithm produced it, so both come from the request; the ciphertext carries
/// no additional authenticated data either, since "the standard asymmetric encryption
/// algorithms ... that KMS uses do not support an encryption context"
/// (`smithy_query(query="doc",shape="DecryptRequest",member="EncryptionContext")`).
///
/// The RSA-OAEP decryption comes from `aws-lc-rs` (`aws_lc_rs::rsa`), under the private
/// key this store holds, with no label — the parameter set
/// [`rsa_oaep_algorithm`] names for the algorithm. Nothing cryptographic is composed
/// here.
pub fn decrypt_asymmetric(
    key: &Cmk,
    encryption_algorithm: &str,
    ciphertext: &[u8],
) -> Result<Vec<u8>, AsymmetricRefusal> {
    let (Some(private_key), Some(key_pair)) = (
        key.material.private_key_pkcs8(),
        key_pair_kind(&key.key_spec),
    ) else {
        return Err(AsymmetricRefusal::Unsupported);
    };

    match key_pair {
        KeyPairKind::Rsa(_) => {
            let oaep_algorithm =
                rsa_oaep_algorithm(encryption_algorithm).ok_or(AsymmetricRefusal::Unsupported)?;
            let private_key = ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(private_key)
                .map_err(|_| AsymmetricRefusal::Unsupported)?;
            let key_size_bytes = private_key.key_size_bytes();
            let private_key = ::aws_lc_rs::rsa::OaepPrivateDecryptingKey::new(private_key)
                .map_err(|_| AsymmetricRefusal::Unsupported)?;
            // An OAEP plaintext is shorter than the modulus the ciphertext occupies,
            // so one modulus of room always holds it.
            let mut plaintext = vec![0u8; key_size_bytes];
            let len = private_key
                .decrypt(oaep_algorithm, ciphertext, &mut plaintext, None)
                .map_err(|_| AsymmetricRefusal::NotThisCiphertext)?
                .len();
            plaintext.truncate(len);
            Ok(plaintext)
        }
        // SM2PKE, the public-key encryption of GM/T 0003-2012, is the one encryption
        // scheme no library reachable from here performs; the remaining key pairs are
        // signing and key-agreement pairs, whose specs advertise no encryption
        // algorithm at all, so a request naming one is refused before it reaches here.
        KeyPairKind::Sm2 | KeyPairKind::Ecdsa(_) | KeyPairKind::Ed25519 => {
            Err(AsymmetricRefusal::Unsupported)
        }
    }
}

/// The RSA-OAEP parameters an `EncryptionAlgorithmSpec` names: the OAEP scheme of RFC
/// 8017 with the digest the algorithm's own name carries, used both as the message
/// digest and as the hash of MGF1.
///
/// `None` for an algorithm that is not one an RSA key pair encrypts under
/// ([`RSA_ENCRYPTION_ALGORITHMS`] is that set).
fn rsa_oaep_algorithm(
    encryption_algorithm: &str,
) -> Option<&'static ::aws_lc_rs::rsa::OaepAlgorithm> {
    match encryption_algorithm {
        ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_1 => Some(&::aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1),
        ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_256 => Some(&::aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256),
        _ => None,
    }
}

/// The ciphertext of `plaintext` under the PUBLIC half of `key`'s key pair, with
/// `encryption_algorithm` — the other direction of [`decrypt_asymmetric`], for the caller
/// that asks KMS to encrypt rather than holding the published public key itself.
///
/// The public half is COMPUTED from the private half this store holds, by the same
/// library that implements the scheme, so what seals a plaintext here is the public key
/// `GetPublicKey` publishes ([`public_key_spki`]) and nothing else can open the result.
/// The ciphertext carries no additional authenticated data: "the standard asymmetric
/// encryption algorithms ... that KMS uses do not support an encryption context"
/// (`smithy_query(query="doc",shape="DecryptRequest",member="EncryptionContext")`).
///
/// The plaintext must already be within the scheme's ceiling
/// ([`max_asymmetric_plaintext_len`]), which is a documented property of the key spec and
/// the algorithm and so is a REFUSAL the handler answers, not a failure here.
pub fn encrypt_asymmetric(
    key: &Cmk,
    encryption_algorithm: &str,
    plaintext: &[u8],
) -> Result<Vec<u8>, AsymmetricRefusal> {
    let (Some(private_key), Some(key_pair)) = (
        key.material.private_key_pkcs8(),
        key_pair_kind(&key.key_spec),
    ) else {
        return Err(AsymmetricRefusal::Unsupported);
    };

    match key_pair {
        KeyPairKind::Rsa(_) => {
            let oaep_algorithm =
                rsa_oaep_algorithm(encryption_algorithm).ok_or(AsymmetricRefusal::Unsupported)?;
            let private_key = ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(private_key)
                .map_err(|_| AsymmetricRefusal::Unsupported)?;
            // An RSA ciphertext is exactly one modulus wide.
            let mut ciphertext = vec![0u8; private_key.key_size_bytes()];
            let public_key = ::aws_lc_rs::rsa::OaepPublicEncryptingKey::new(private_key.public_key())
                .map_err(|_| AsymmetricRefusal::Unsupported)?;
            let len = public_key
                .encrypt(oaep_algorithm, plaintext, &mut ciphertext, None)
                .map_err(|_| AsymmetricRefusal::Failed)?
                .len();
            ciphertext.truncate(len);
            Ok(ciphertext)
        }
        // The same scheme coverage [`decrypt_asymmetric`] has: SM2PKE is the one
        // encryption scheme no library reachable from here performs, and the remaining
        // key pairs advertise no encryption algorithm at all.
        KeyPairKind::Sm2 | KeyPairKind::Ecdsa(_) | KeyPairKind::Ed25519 => {
            Err(AsymmetricRefusal::Unsupported)
        }
    }
}

/// The largest plaintext an asymmetric KMS key of `key_spec` encrypts under
/// `encryption_algorithm`, in bytes, or `None` when the pair names no scheme with a
/// ceiling this computes.
///
/// For RSAES-OAEP the ceiling is the scheme's own: a message of at most `k - 2*hLen - 2`
/// bytes, where `k` is the modulus in bytes and `hLen` the digest's output length (RFC
/// 8017 §7.1.1). It is therefore derived from the KEY SPEC and the ALGORITHM rather than
/// listed, and it reproduces every ceiling the operation documents — "RSA_2048
/// RSAES_OAEP_SHA_1: 214 bytes RSAES_OAEP_SHA_256: 190 bytes RSA_3072 RSAES_OAEP_SHA_1:
/// 342 bytes RSAES_OAEP_SHA_256: 318 bytes RSA_4096 RSAES_OAEP_SHA_1: 470 bytes
/// RSAES_OAEP_SHA_256: 446 bytes" (`smithy_query(query="doc",shape="Encrypt")`).
pub fn max_asymmetric_plaintext_len(key_spec: &str, encryption_algorithm: &str) -> Option<usize> {
    let KeyPairKind::Rsa(modulus_bits) = key_pair_kind(key_spec)? else {
        return None;
    };
    let digest_len = match encryption_algorithm {
        ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_1 => ::aws_lc_rs::digest::SHA1_OUTPUT_LEN,
        ENCRYPTION_ALGORITHM_RSAES_OAEP_SHA_256 => ::aws_lc_rs::digest::SHA256_OUTPUT_LEN,
        _ => return None,
    };
    (modulus_bits / 8).checked_sub(2 * digest_len + 2)
}

// ---------------------------------------------------------------------------
// Digital signatures
// ---------------------------------------------------------------------------

/// The `MessageType` of a message that is signed as it was given, hashed by the
/// signing algorithm itself: "use RAW for unhashed messages"
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`), and the
/// value KMS assumes when a request names none.
pub const MESSAGE_TYPE_RAW: &str = "RAW";

/// The `MessageType` of a message that is ALREADY the hash the signing algorithm
/// would have computed: "when the value is DIGEST, KMS skips the hashing step in the
/// signing algorithm" (the same documentation). See [`MESSAGE_TYPE_RAW`].
pub const MESSAGE_TYPE_DIGEST: &str = "DIGEST";

/// The `MessageType` of the "64-byte representative μ used in ML-DSA signing as
/// defined in NIST FIPS 204 Section 6.2" (the same documentation), which is therefore
/// a message type the ML-DSA signing algorithm alone takes. See [`MESSAGE_TYPE_RAW`].
pub const MESSAGE_TYPE_EXTERNAL_MU: &str = "EXTERNAL_MU";

/// The `SigningAlgorithmSpec` of pure Ed25519 (`smithy_query(shape="SigningAlgorithmSpec")`),
/// which hashes the message inside the signature scheme itself (RFC 8032) and so
/// "requires KMS MessageType:RAW"
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`).
pub const SIGNING_ALGORITHM_ED25519_SHA_512: &str = "ED25519_SHA_512";

/// The `SigningAlgorithmSpec` of prehashed Ed25519, which "requires KMS
/// MessageType:DIGEST" (the same documentation). See
/// [`SIGNING_ALGORITHM_ED25519_SHA_512`].
pub const SIGNING_ALGORITHM_ED25519_PH_SHA_512: &str = "ED25519_PH_SHA_512";

/// The `SigningAlgorithmSpec` of ML-DSA, the one algorithm whose message may be an
/// external μ. See [`SIGNING_ALGORITHM_ED25519_SHA_512`].
pub const SIGNING_ALGORITHM_ML_DSA_SHAKE_256: &str = "ML_DSA_SHAKE_256";

/// The hash a `SigningAlgorithmSpec` hashes the message with, which is stated in the
/// algorithm's own name: "signing algorithms that end in SHA_256 use the SHA_256
/// hashing algorithm", and the same for SHA_384 and SHA_512
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`). It is also
/// the length a message of [`MESSAGE_TYPE_DIGEST`] must have, because such a message
/// is that hash already computed.
///
/// `None` for an algorithm the same documentation gives another hash — the SHAKE_256
/// of ML-DSA and the SM3 of SM2DSA.
pub fn signing_hash(signing_algorithm: &str) -> Option<&'static ::aws_lc_rs::digest::Algorithm> {
    use ::aws_lc_rs::digest::{SHA256, SHA384, SHA512};

    [
        ("SHA_256", &SHA256),
        ("SHA_384", &SHA384),
        ("SHA_512", &SHA512),
    ]
    .into_iter()
    .find_map(|(suffix, hash)| signing_algorithm.ends_with(suffix).then_some(hash))
}

/// Whether `signing_algorithm` signs a message of `message_type` at all.
///
/// An external μ is "used in ML-DSA signing", so ML-DSA is the only algorithm that
/// takes one; each Edwards25519 algorithm takes exactly one message type
/// ("ED25519_SHA_512 signing algorithm requires KMS MessageType:RAW",
/// "ED25519_PH_SHA_512 signing algorithm requires KMS MessageType:DIGEST"); and every
/// other algorithm signs either the message or its digest, since it "begins with a
/// hash function" that `DIGEST` skips (all quoted from
/// `smithy_query(query="doc",shape="SignRequest",member="MessageType")`).
pub fn signs_message_type(signing_algorithm: &str, message_type: &str) -> bool {
    if let Some(required) = required_message_type(signing_algorithm) {
        return message_type == required;
    }
    if signing_algorithm == SIGNING_ALGORITHM_ML_DSA_SHAKE_256 {
        return [
            MESSAGE_TYPE_RAW,
            MESSAGE_TYPE_DIGEST,
            MESSAGE_TYPE_EXTERNAL_MU,
        ]
        .contains(&message_type);
    }
    [MESSAGE_TYPE_RAW, MESSAGE_TYPE_DIGEST].contains(&message_type)
}

/// The one `MessageType` a signing algorithm requires, for an algorithm that takes
/// exactly one: "ED25519_SHA_512 signing algorithm requires KMS MessageType:RAW" and
/// "ED25519_PH_SHA_512 signing algorithm requires KMS MessageType:DIGEST"
/// (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`). `None` for
/// an algorithm that signs either the message or its digest, which is every other one.
///
/// It is the table [`signs_message_type`] judges a request against, so the predicate
/// and the refusal that names the requirement cannot disagree.
pub fn required_message_type(signing_algorithm: &str) -> Option<&'static str> {
    match signing_algorithm {
        SIGNING_ALGORITHM_ED25519_SHA_512 => Some(MESSAGE_TYPE_RAW),
        SIGNING_ALGORITHM_ED25519_PH_SHA_512 => Some(MESSAGE_TYPE_DIGEST),
        _ => None,
    }
}

/// Why no signature could be produced for a request the key and the algorithm are
/// otherwise right for.
#[derive(Debug)]
pub enum SignatureRefusal {
    /// No library reachable from here signs with this key pair under this algorithm,
    /// so there are no signature bytes to answer with. The KMS key is still real —
    /// the caller is told the operation is not written yet rather than handed bytes
    /// that are not a signature of its message.
    Unsupported,
    /// The library refused to produce a signature it should have produced, which is a
    /// server fault rather than anything about the request.
    Failed,
}

/// The signature of `message` under the PRIVATE half of `key`'s key pair, with
/// `signing_algorithm`.
///
/// `message_type` says what `message` is: [`MESSAGE_TYPE_RAW`] is the message, which
/// is hashed with the algorithm's own hash ([`signing_hash`]) before it is signed, and
/// [`MESSAGE_TYPE_DIGEST`] is that hash already computed, which is signed as it
/// stands — "when the value is DIGEST, KMS skips the hashing step in the signing
/// algorithm" (`smithy_query(query="doc",shape="SignRequest",member="MessageType")`).
///
/// The bytes are the encoding the algorithm is defined to produce, which is what the
/// `Signature` member carries: "the encoding of this value is defined by PKCS #1 in
/// RFC 8017" for the RSA algorithms and "a DER-encoded object as defined by ANSI
/// X9.62-2005 and RFC 3279 Section 2.2.3" for the ECDSA ones
/// (`smithy_query(query="doc",shape="SignResponse",member="Signature")`). The private
/// key is only read here, never copied out: it is the library that signs, and this
/// function composes nothing cryptographic of its own.
pub fn sign_message(
    key: &Cmk,
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
) -> Result<Vec<u8>, SignatureRefusal> {
    use ::aws_lc_rs::signature::{EcdsaKeyPair, Ed25519KeyPair, RsaKeyPair};

    let (Some(private_key), Some(key_pair)) = (
        key.material.private_key_pkcs8(),
        key_pair_kind(&key.key_spec),
    ) else {
        return Err(SignatureRefusal::Unsupported);
    };

    match key_pair {
        KeyPairKind::Rsa(_) => {
            let encoding =
                rsa_signing_encoding(signing_algorithm).ok_or(SignatureRefusal::Unsupported)?;
            let digest = signed_digest(signing_algorithm, message_type, message)?;
            let pair =
                RsaKeyPair::from_pkcs8(private_key).map_err(|_| SignatureRefusal::Failed)?;
            // A PKCS #1 signature is exactly as long as the key's modulus.
            let mut signature = vec![0u8; pair.public_modulus_len()];
            pair.sign_digest(encoding, &digest, &mut signature)
                .map_err(|_| SignatureRefusal::Failed)?;
            Ok(signature)
        }
        KeyPairKind::Ecdsa(curve) => {
            let digest = signed_digest(signing_algorithm, message_type, message)?;
            let pair = EcdsaKeyPair::from_pkcs8(curve, private_key)
                .map_err(|_| SignatureRefusal::Failed)?;
            Ok(pair
                .sign_digest(&digest)
                .map_err(|_| SignatureRefusal::Failed)?
                .as_ref()
                .to_vec())
        }
        // Pure Ed25519 hashes the message within the signature scheme itself (RFC
        // 8032), so what it signs is the message and not a digest of it.
        KeyPairKind::Ed25519 if signing_algorithm == SIGNING_ALGORITHM_ED25519_SHA_512 => {
            let pair =
                Ed25519KeyPair::from_pkcs8(private_key).map_err(|_| SignatureRefusal::Failed)?;
            Ok(pair.sign(message).as_ref().to_vec())
        }
        // The prehashed Ed25519 scheme of FIPS 186-5 Section 7.8.1 and the SM2DSA of
        // GM/T 0003-2012 are the two signature schemes no library reachable from here
        // produces a signature with.
        KeyPairKind::Ed25519 | KeyPairKind::Sm2 => Err(SignatureRefusal::Unsupported),
    }
}

/// The digest `signing_algorithm` signs for a message of `message_type`: the hash of
/// the message, or the hash the caller already computed, taken as the digest it says
/// it is.
fn signed_digest(
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
) -> Result<::aws_lc_rs::digest::Digest, SignatureRefusal> {
    use ::aws_lc_rs::digest::{digest, Digest};

    let hash = signing_hash(signing_algorithm).ok_or(SignatureRefusal::Unsupported)?;
    if message_type == MESSAGE_TYPE_DIGEST {
        // The caller's bytes ARE the digest, so the only thing to check is that they
        // are as many bytes as that hash produces — which the caller checked before
        // reaching here, and the library checks again.
        return Digest::import_less_safe(message, hash).map_err(|_| SignatureRefusal::Failed);
    }
    Ok(digest(hash, message))
}

/// The RSA signature scheme a `SigningAlgorithmSpec` names: the RSASSA-PKCS1-v1_5 or
/// the RSASSA-PSS scheme of RFC 8017, with the hash the algorithm's name carries.
/// `None` for an algorithm that is not one an RSA key pair signs with
/// ([`RSA_SIGNING_ALGORITHMS`] is that set).
fn rsa_signing_encoding(
    signing_algorithm: &str,
) -> Option<&'static dyn ::aws_lc_rs::signature::RsaEncoding> {
    use ::aws_lc_rs::signature::{
        RSA_PKCS1_SHA256, RSA_PKCS1_SHA384, RSA_PKCS1_SHA512, RSA_PSS_SHA256, RSA_PSS_SHA384,
        RSA_PSS_SHA512,
    };

    match signing_algorithm {
        "RSASSA_PKCS1_V1_5_SHA_256" => Some(&RSA_PKCS1_SHA256),
        "RSASSA_PKCS1_V1_5_SHA_384" => Some(&RSA_PKCS1_SHA384),
        "RSASSA_PKCS1_V1_5_SHA_512" => Some(&RSA_PKCS1_SHA512),
        "RSASSA_PSS_SHA_256" => Some(&RSA_PSS_SHA256),
        "RSASSA_PSS_SHA_384" => Some(&RSA_PSS_SHA384),
        "RSASSA_PSS_SHA_512" => Some(&RSA_PSS_SHA512),
        _ => None,
    }
}

/// Whether `signature` is a signature of `message` under `key`'s key pair with
/// `signing_algorithm`.
///
/// "The signature is verified by using the public key in the same asymmetric KMS key"
/// (`smithy_query(query="doc",shape="Verify")`): the check is performed by the library
/// that implements the algorithm, against the public half DERIVED from the private half
/// this store holds ([`public_key_spki`]) — the same bytes `GetPublicKey` publishes — so
/// a signature verifies here exactly when it verifies outside KMS, and a signature of
/// another message, under another key or with another algorithm does not verify.
///
/// `message_type` says what `message` is, as it does for [`sign_message`]: a message of
/// [`MESSAGE_TYPE_RAW`] is hashed by the scheme itself, and one of
/// [`MESSAGE_TYPE_DIGEST`] is the hash already computed, so the signature is checked
/// over that digest with the hashing step skipped — "a message and its hash digest are
/// considered to be the same message"
/// (`smithy_query(query="doc",shape="VerifyRequest",member="Message")`).
///
/// The errors mean what they mean for [`sign_message`]: `Unsupported` when no library
/// reachable from here verifies this key pair under this algorithm, and `Failed` when a
/// library refused a step it should have performed.
pub fn signature_matches(
    key: &Cmk,
    signing_algorithm: &str,
    message_type: &str,
    message: &[u8],
    signature: &[u8],
) -> Result<bool, SignatureRefusal> {
    let (Some(private_key), Some(algorithm)) = (
        key.material.private_key_pkcs8(),
        verification_algorithm(&key.key_spec, signing_algorithm),
    ) else {
        return Err(SignatureRefusal::Unsupported);
    };

    let public_key =
        public_key_spki(&key.key_spec, private_key).ok_or(SignatureRefusal::Failed)?;
    let parsed = ::aws_lc_rs::signature::ParsedPublicKey::new(algorithm, &public_key)
        .map_err(|_| SignatureRefusal::Failed)?;
    if message_type == MESSAGE_TYPE_DIGEST {
        let digest = signed_digest(signing_algorithm, message_type, message)?;
        return Ok(parsed.verify_digest_sig(&digest, signature).is_ok());
    }
    Ok(parsed.verify_sig(message, signature).is_ok())
}

/// How a signature produced with `signing_algorithm` under a key pair of `key_spec` is
/// VERIFIED: the same scheme, hash and curve, named from the verifying side.
///
/// The RSA schemes are verified for any modulus the specs allow, and each curve spec
/// names its own curve, so the parameters a signature is checked with are the key's own
/// rather than the ones a request asked for. `None` for a pairing no library reachable
/// from here verifies — the prehashed Ed25519 scheme and SM2DSA, which [`sign_message`]
/// likewise produces no signature with, and ML-DSA, whose key pair this service does not
/// hold.
fn verification_algorithm(
    key_spec: &str,
    signing_algorithm: &str,
) -> Option<&'static dyn ::aws_lc_rs::signature::VerificationAlgorithm> {
    use ::aws_lc_rs::signature::{
        ECDSA_P256K1_SHA256_ASN1, ECDSA_P256_SHA256_ASN1, ECDSA_P384_SHA384_ASN1,
        ECDSA_P521_SHA512_ASN1, ED25519, RSA_PKCS1_2048_8192_SHA256, RSA_PKCS1_2048_8192_SHA384,
        RSA_PKCS1_2048_8192_SHA512, RSA_PSS_2048_8192_SHA256, RSA_PSS_2048_8192_SHA384,
        RSA_PSS_2048_8192_SHA512,
    };

    match key_pair_kind(key_spec)? {
        KeyPairKind::Rsa(_) => match signing_algorithm {
            "RSASSA_PKCS1_V1_5_SHA_256" => Some(&RSA_PKCS1_2048_8192_SHA256),
            "RSASSA_PKCS1_V1_5_SHA_384" => Some(&RSA_PKCS1_2048_8192_SHA384),
            "RSASSA_PKCS1_V1_5_SHA_512" => Some(&RSA_PKCS1_2048_8192_SHA512),
            "RSASSA_PSS_SHA_256" => Some(&RSA_PSS_2048_8192_SHA256),
            "RSASSA_PSS_SHA_384" => Some(&RSA_PSS_2048_8192_SHA384),
            "RSASSA_PSS_SHA_512" => Some(&RSA_PSS_2048_8192_SHA512),
            _ => None,
        },
        KeyPairKind::Ecdsa(_) => match (key_spec, signing_algorithm) {
            ("ECC_NIST_P256", "ECDSA_SHA_256") => Some(&ECDSA_P256_SHA256_ASN1),
            ("ECC_NIST_P384", "ECDSA_SHA_384") => Some(&ECDSA_P384_SHA384_ASN1),
            ("ECC_NIST_P521", "ECDSA_SHA_512") => Some(&ECDSA_P521_SHA512_ASN1),
            ("ECC_SECG_P256K1", "ECDSA_SHA_256") => Some(&ECDSA_P256K1_SHA256_ASN1),
            _ => None,
        },
        KeyPairKind::Ed25519 if signing_algorithm == SIGNING_ALGORITHM_ED25519_SHA_512 => {
            Some(&ED25519)
        }
        KeyPairKind::Ed25519 | KeyPairKind::Sm2 => None,
    }
}

// ---------------------------------------------------------------------------
// Message authentication codes
// ---------------------------------------------------------------------------

/// The HMAC algorithm a `MacAlgorithmSpec` names.
///
/// "The HMAC KMS keys and the HMAC algorithms that KMS uses conform to industry
/// standards defined in RFC 2104" (`smithy_query(query="doc",shape="GenerateMac")`),
/// and the spec's own name gives the hash that HMAC is built on, so `HMAC_SHA_384` is
/// HMAC-SHA-384. `None` for a value that is not one of the four the model declares
/// (`smithy_query(query="shape",shape="MacAlgorithmSpec")`), which the modelled enum
/// constraint already refuses before a handler runs.
pub fn mac_hash(mac_algorithm: &str) -> Option<::aws_lc_rs::hmac::Algorithm> {
    use ::aws_lc_rs::hmac::{HMAC_SHA224, HMAC_SHA256, HMAC_SHA384, HMAC_SHA512};

    match mac_algorithm {
        "HMAC_SHA_224" => Some(HMAC_SHA224),
        "HMAC_SHA_256" => Some(HMAC_SHA256),
        "HMAC_SHA_384" => Some(HMAC_SHA384),
        "HMAC_SHA_512" => Some(HMAC_SHA512),
        _ => None,
    }
}

/// The HMAC of `message` under `key`'s own key material with `mac_algorithm`: "the
/// standard, raw HMAC defined in RFC 2104"
/// (`smithy_query(query="doc",shape="GenerateMacResponse",member="Mac")`).
///
/// The tag is produced by the library that implements the algorithm, from the bytes
/// this key holds — which never leave this process — so it is a function of the key,
/// the algorithm and the message and of nothing else: the same three inputs give the
/// same tag, and any change to one of them gives another.
///
/// `None` when the key holds no HMAC key material, or `mac_algorithm` names no HMAC
/// algorithm: there is then no HMAC of the caller's message to answer with.
pub fn generate_mac(key: &Cmk, mac_algorithm: &str, message: &[u8]) -> Option<Vec<u8>> {
    let hmac_key = mac_key(key, mac_algorithm)?;
    Some(::aws_lc_rs::hmac::sign(&hmac_key, message).as_ref().to_vec())
}

/// Whether `mac` is the HMAC of `message` under `key`'s material with `mac_algorithm`.
///
/// "VerifyMac computes an HMAC using the message, HMAC KMS key, and MAC algorithm that
/// you specify, and compares the computed HMAC to the HMAC that you specify"
/// (`smithy_query(query="doc",shape="VerifyMac")`), which is the comparison the library
/// itself performs — in constant time, and false for a tag of the wrong length rather
/// than a panic. A tag from another key, another algorithm or another message therefore
/// does not verify.
pub fn mac_matches(key: &Cmk, mac_algorithm: &str, message: &[u8], mac: &[u8]) -> bool {
    mac_key(key, mac_algorithm)
        .is_some_and(|hmac_key| ::aws_lc_rs::hmac::verify(&hmac_key, message, mac).is_ok())
}

/// `key`'s HMAC key material as a key of the HMAC algorithm `mac_algorithm` names, so
/// generating a MAC and verifying one are the same key over the same bytes.
///
/// The material is the key's CURRENT material ([`Cmk::effective_material`]), which is
/// what a cryptographic operation on the key uses.
fn mac_key(key: &Cmk, mac_algorithm: &str) -> Option<::aws_lc_rs::hmac::Key> {
    let KeyMaterial::Hmac(material) = key.effective_material() else {
        return None;
    };
    Some(::aws_lc_rs::hmac::Key::new(
        mac_hash(mac_algorithm)?,
        material,
    ))
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

/// The order a grant's permitted operations are reported in: the order the model
/// DECLARES the `GrantOperation` enum's values in
/// (`smithy_query(query="shape",shape="GrantOperation")`, whose `values` are listed in
/// declaration order).
///
/// `GrantOperationList` is a plain list of that enum, so the model fixes no order for
/// it, and the recorded AWS wire reports a grant's operations in this one whatever
/// order the creating request listed them in. Projecting through it is what makes two
/// readings of one grant, and two grants permitting the same set, render identically.
pub const GRANT_OPERATION_ORDER: &[&str] = &[
    "Decrypt",
    "Encrypt",
    "GenerateDataKey",
    "GenerateDataKeyWithoutPlaintext",
    "ReEncryptFrom",
    "ReEncryptTo",
    "Sign",
    "Verify",
    "GetPublicKey",
    "CreateGrant",
    "RetireGrant",
    "DescribeKey",
    "GenerateDataKeyPair",
    "GenerateDataKeyPairWithoutPlaintext",
    "GenerateMac",
    "VerifyMac",
    "DeriveSharedSecret",
];

/// Where `operation` comes in [`GRANT_OPERATION_ORDER`] — past the end for a value the
/// enum does not declare, which the modelled enum constraint refuses before a handler
/// runs, so such a value keeps the place the request gave it rather than being dropped.
pub fn grant_operation_rank(operation: &str) -> usize {
    GRANT_OPERATION_ORDER
        .iter()
        .position(|declared| *declared == operation)
        .unwrap_or(GRANT_OPERATION_ORDER.len())
}

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
    ///
    /// `Name` is ALWAYS reported: the name the grant was created with, or the empty
    /// string for a grant created without one, so a client reading it never has to
    /// tell a missing member from a grant that has no name. The member's own
    /// documentation says the value "is null" for a grant created without a name
    /// (`smithy_query(query="doc",shape="GrantListEntry",member="Name")`), but the
    /// recorded AWS wire (`simparity-wire-aws-v1`, the `kms-grant-*` fixtures)
    /// reports the empty string, so that is what a reader answers.
    ///
    /// The permitted operations are reported in the order the `GrantOperation` enum
    /// declares its values ([`GRANT_OPERATION_ORDER`]) rather than in the order the
    /// creating request listed them: they are a SET of permissions (which is also how
    /// a retry of `CreateGrant` compares them, [`Self::fingerprint`]), so the
    /// projection puts them in one canonical order and a client comparing two readings
    /// of the same grant sees no difference.
    pub fn list_entry(&self) -> GrantListEntry {
        let mut operations = self.operations.clone();
        operations.sort_by_key(|operation| grant_operation_rank(operation));
        GrantListEntry {
            key_id: Some(self.key_arn.clone()),
            grant_id: Some(self.grant_id.clone()),
            name: Some(self.name.clone().unwrap_or_default()),
            creation_date: Some(date_time(self.creation_date)),
            grantee_principal: self.grantee_principal.clone(),
            retiring_principal: self.retiring_principal.clone(),
            issuing_account: Some(self.issuing_account.clone()),
            operations: Some(operations),
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
/// KMS will not confirm whether a grant exists to a caller that cannot retire it:
/// "permission to retire a grant is determined primarily by the grant"
/// (`smithy_query(query="doc",shape="RetireGrant")`), so with no such grant there is no
/// permission to retire one — and the refusal is the authorization failure that reports
/// nothing about the resource, naming the caller and the action alone
/// ([`unauthorized_resource_message`]).
pub fn retire_grant_access_denied_message(principal: &str, operation: &str) -> String {
    unauthorized_resource_message(principal, operation)
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
/// (`smithy_query(query="doc",shape="CreateGrantRequest",member="GranteePrincipal")`),
/// and the same for the retiring principal `ListRetirableGrants` and `CreateGrant` take:
/// "valid principals include Amazon Web Services accounts, IAM users, IAM roles,
/// federated users, and assumed role users"
/// (`smithy_query(query="doc",shape="ListRetirableGrantsRequest",member="RetiringPrincipal")`).
///
/// `PrincipalIdType`'s modelled pattern accepts any word-ish string, so the IDENTITY form
/// is what is left to check — and it is the same union of forms every other service reads
/// a policy principal against, so it is the tree's one reading of it
/// ([`::service_common::arn::is_iam_principal_arn`]) rather than a second grammar here: an
/// ARN whose resource field names no IAM or STS identity (no resource type at all, or one
/// that is not an identity's) denotes no principal, which is what the operations model
/// `InvalidArnException` for.
pub fn names_a_principal_arn(principal: &str) -> bool {
    ::service_common::arn::is_iam_principal_arn(principal)
}

/// How a value that names no AWS principal is refused: `InvalidArnException`'s message
/// names the value the request gave, since that string is the whole of what is wrong with
/// it.
///
/// KMS words this refusal of a PRINCIPAL differently from the one it words for a key
/// identifier ([`invalid_arn_message`]) — the recorded wording names the ARN with no
/// sentence around it — so the two live side by side here rather than one being used for
/// both. One function for every member that carries a principal, so no two operations
/// word the same refusal differently.
pub fn invalid_principal_arn_message(requested: &str) -> String {
    format!("ARN is not valid: {requested}")
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

/// How KMS words `InvalidMarkerException`, "the marker that specifies where pagination
/// should next begin is not valid", on the listings whose refusal of one is recorded
/// carrying no message.
///
/// KMS refuses a marker it never issued with the error CODE alone — the refusal is
/// recorded on the wire with no `Message` member at all — so no explanatory text is
/// manufactured for it. What was wrong with the marker is stated in a comment at the
/// check that refuses it, not on the wire.
///
/// One function for every paginated operation that answers it that way, so no two of them
/// answer the same refusal with different members — which is why it is a function
/// returning `None` rather than `None` written out at each call site. The TAG listing is
/// recorded naming the token instead ([`invalid_next_marker_message`]).
#[must_use]
pub fn invalid_marker_message() -> Option<String> {
    None
}

/// How the TAG listing refuses a `Marker` it did not issue: by naming the token it
/// rejected, `Invalid NextMarker: <marker>`.
///
/// A `Marker` "specifies where pagination should next begin"
/// (`smithy_query(query="doc",shape="ListResourceTagsRequest",member="Marker")`), and a
/// token this reader never minted — or one naming a tag the key no longer carries —
/// resumes nowhere; the token is the whole of what the request got wrong, so it is the
/// refusal's one variable part and it travels back exactly as the request supplied it.
///
/// It is the tagging surface's own wording: the recorded refusal of a marker by the key,
/// alias and grant listings carries no message at all ([`invalid_marker_message`]), so
/// each surface keeps the wording its own refusal is recorded with rather than borrowing
/// the other's.
#[must_use]
pub fn invalid_next_marker_message(marker: &str) -> String {
    format!("Invalid NextMarker: {marker}")
}

/// [`MARKER_PREFIX`], for the custom key store list, which pages over custom key
/// stores rather than keys and so mints its own tokens.
const CUSTOM_KEY_STORE_MARKER_PREFIX: &str = "custom-key-stores/1/";

/// [`encode_marker`], for the custom key store list: the token resumes after the
/// key store whose id is `custom_key_store_id`.
pub fn encode_custom_key_store_marker(custom_key_store_id: &str) -> String {
    encode_marker_for(CUSTOM_KEY_STORE_MARKER_PREFIX, custom_key_store_id)
}

/// [`decode_marker`], for the custom key store list.
pub fn decode_custom_key_store_marker(marker: &str) -> Option<String> {
    decode_marker_for(CUSTOM_KEY_STORE_MARKER_PREFIX, marker)
}

/// [`MARKER_PREFIX`], for the key-material rotation list, which pages over the
/// versions of one key's material and so mints its own tokens.
const ROTATION_MARKER_PREFIX: &str = "key-rotations/1/";

/// [`encode_marker`], for the rotation list: the token resumes after the key material
/// whose `KeyMaterialId` is `key_material_id`.
pub fn encode_rotation_marker(key_material_id: &str) -> String {
    encode_marker_for(ROTATION_MARKER_PREFIX, key_material_id)
}

/// [`decode_marker`], for the rotation list.
pub fn decode_rotation_marker(marker: &str) -> Option<String> {
    decode_marker_for(ROTATION_MARKER_PREFIX, marker)
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

// ---------------------------------------------------------------------------
// Importing key material
// ---------------------------------------------------------------------------

/// The `WrappingKeySpec` values this service generates a wrapping key pair for, from
/// `smithy_query(query="shape",shape="WrappingKeySpec")`. `SM2` is the fourth value
/// and is not one of these: see [`wrapping_key_size`].
pub const WRAPPING_KEY_SPEC_RSA_2048: &str = "RSA_2048";
/// See [`WRAPPING_KEY_SPEC_RSA_2048`].
pub const WRAPPING_KEY_SPEC_RSA_3072: &str = "RSA_3072";
/// See [`WRAPPING_KEY_SPEC_RSA_2048`].
pub const WRAPPING_KEY_SPEC_RSA_4096: &str = "RSA_4096";

/// The RSA key size a `WrappingKeySpec` names, or `None` for a spec that is not an RSA
/// key pair this service generates.
///
/// The accepted specs are the catalog above; the modulus size is read from the spec's
/// own name through [`rsa_key_size`], the one place a modulus size becomes a key size.
fn wrapping_key_size(wrapping_key_spec: &str) -> Option<::aws_lc_rs::rsa::KeySize> {
    match wrapping_key_spec {
        WRAPPING_KEY_SPEC_RSA_2048 | WRAPPING_KEY_SPEC_RSA_3072 | WRAPPING_KEY_SPEC_RSA_4096 => {
            rsa_key_size(wrapping_key_spec.strip_prefix("RSA_")?.parse().ok()?)
        }
        _ => None,
    }
}

/// Whether this service can generate a wrapping key pair of `wrapping_key_spec`.
pub fn generates_wrapping_key(wrapping_key_spec: &str) -> bool {
    wrapping_key_size(wrapping_key_spec).is_some()
}

/// A freshly generated wrapping key pair of the kind `wrapping_key_spec` names: the
/// public half as X.509 `SubjectPublicKeyInfo` DER, which is what the response carries,
/// and the private half as PKCS#8 DER, which is what this store keeps.
///
/// The RSA key pair comes from `aws-lc-rs` (`aws_lc_rs::rsa`) rather than from anything
/// written here, and the public half really is the public half of the private one, so
/// key material wrapped with it is material `ImportKeyMaterial` can open.
///
/// `None` means the platform refused to generate a key pair; the caller answers a
/// modelled server error rather than returning a public key with no private half.
pub fn mint_wrapping_key(wrapping_key_spec: &str) -> Option<(Vec<u8>, Vec<u8>)> {
    use ::aws_lc_rs::encoding::AsDer;

    let key_size = wrapping_key_size(wrapping_key_spec)?;
    let private_key = ::aws_lc_rs::rsa::PrivateDecryptingKey::generate(key_size).ok()?;
    let public_key = private_key.public_key().as_der().ok()?.as_ref().to_vec();
    let private_key_pkcs8 = private_key.as_der().ok()?.as_ref().to_vec();
    Some((public_key, private_key_pkcs8))
}

// The matching UNWRAP of key material a caller sealed under the public half of a
// wrapping key pair is [`unwrap_key_material`] below.

/// Open key material a caller wrapped under the public half of `parameters`' wrapping
/// key pair, with the wrapping algorithm those parameters were issued for.
///
/// The RSA-OAEP decryption comes from `aws-lc-rs` (`aws_lc_rs::rsa`), under the private
/// half this store kept when the public half was handed out, so what comes back is
/// exactly the material the caller wrapped. An `RSA_AES` wrapping has a second step:
/// what the RSA public key encrypted is an ephemeral AES key, and that key wrapped the
/// material — see [`Wrapping`].
///
/// `None` means the bytes are not key material those parameters can open: either the
/// wrapping algorithm is one this service does not unwrap (see
/// [`unwraps_wrapping_algorithm`]) or the unwrapping failed.
pub fn unwrap_key_material(parameters: &ImportParameters, wrapped: &[u8]) -> Option<Vec<u8>> {
    let wrapping = wrapping_of(&parameters.wrapping_algorithm)?;
    let private_key =
        ::aws_lc_rs::rsa::PrivateDecryptingKey::from_pkcs8(&parameters.private_key_pkcs8).ok()?;
    let key_size_bytes = private_key.key_size_bytes();
    let oaep = ::aws_lc_rs::rsa::OaepPrivateDecryptingKey::new(private_key).ok()?;
    // What the RSA public key encrypted is the whole material for an RSAES wrapping and
    // the ephemeral AES key alone for an `RSA_AES` one, in which case it occupies exactly
    // one RSA block and the AES-wrapped material is the rest.
    let (sealed, aes_wrapped) = match wrapping {
        Wrapping::Direct(_) => (wrapped, None),
        Wrapping::AesKeyWrap(_) => {
            if wrapped.len() <= key_size_bytes {
                return None;
            }
            let (block, rest) = wrapped.split_at(key_size_bytes);
            (block, Some(rest))
        }
    };
    let mut opened = vec![0u8; key_size_bytes];
    let len = oaep
        .decrypt(wrapping.oaep_algorithm(), sealed, &mut opened, None)
        .ok()?
        .len();
    opened.truncate(len);
    match aes_wrapped {
        None => Some(opened),
        Some(aes_wrapped) => aes_key_unwrap(&opened, aes_wrapped),
    }
}

/// Open key material wrapped with the ephemeral AES key `kek` under AES key wrap with
/// padding, which is the second step of an `RSA_AES` wrapping.
///
/// "Generate a 256-bit AES symmetric key", "encrypt your key material with the AES key"
/// and "encrypt the AES key with the public key", then concatenate the two
/// (`smithy_query(query="doc",shape="GetParametersForImportRequest",member="WrappingAlgorithm")`),
/// so the material is unwrapped with the AES key the RSA block carried. The key wrap
/// comes from `aws-lc-rs` (`aws_lc_rs::key_wrap`), not from anything written here.
///
/// `None` means the bytes are not material wrapped with that key.
fn aes_key_unwrap(kek: &[u8], wrapped: &[u8]) -> Option<Vec<u8>> {
    use ::aws_lc_rs::key_wrap::KeyWrapPadded;

    let kek = ::aws_lc_rs::key_wrap::AesKek::new(&::aws_lc_rs::key_wrap::AES_256, kek).ok()?;
    let mut opened = vec![0u8; wrapped.len()];
    let len = kek.unwrap_with_padding(wrapped, &mut opened).ok()?.len();
    opened.truncate(len);
    Some(opened)
}

/// Whether this service can unwrap key material wrapped with `wrapping_algorithm`.
pub fn unwraps_wrapping_algorithm(wrapping_algorithm: &str) -> bool {
    wrapping_of(wrapping_algorithm).is_some()
}

/// How a caller wrapped the key material it is importing, which is what the
/// `AlgorithmSpec` of the parameters it names states.
#[derive(Debug, Clone, Copy)]
enum Wrapping {
    /// The material itself is encrypted under the RSA public key: "for RSAES wrapping
    /// algorithms, you encrypt your key material directly with the RSA public key from
    /// KMS", with these OAEP parameters.
    Direct(&'static ::aws_lc_rs::rsa::OaepAlgorithm),
    /// An ephemeral AES key is encrypted under the RSA public key with these OAEP
    /// parameters, and the material is wrapped with that AES key: "for RSA_AES wrapping
    /// algorithms, you encrypt your key material with an AES symmetric key that you
    /// generate, then encrypt your AES key with the RSA public key from KMS".
    AesKeyWrap(&'static ::aws_lc_rs::rsa::OaepAlgorithm),
}

impl Wrapping {
    /// The OAEP parameters the RSA step uses, which both forms of wrapping carry.
    fn oaep_algorithm(&self) -> &'static ::aws_lc_rs::rsa::OaepAlgorithm {
        match self {
            Self::Direct(algorithm) | Self::AesKeyWrap(algorithm) => algorithm,
        }
    }
}

/// The wrapping an `AlgorithmSpec` names, or `None` for a spec that is not one this
/// service unwraps: the four RSA values of
/// `smithy_query(query="shape",shape="AlgorithmSpec")` are wrappings under the RSA key
/// pair `GetParametersForImport` handed out, while `SM2PKE` is not RSA at all.
///
/// The digest in the name is the digest of the OAEP step, for both forms
/// (`smithy_query(query="doc",shape="GetParametersForImportRequest",member="WrappingAlgorithm")`).
fn wrapping_of(wrapping_algorithm: &str) -> Option<Wrapping> {
    match wrapping_algorithm {
        ALGORITHM_SPEC_RSAES_OAEP_SHA_1 => Some(Wrapping::Direct(
            &::aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1,
        )),
        ALGORITHM_SPEC_RSAES_OAEP_SHA_256 => Some(Wrapping::Direct(
            &::aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256,
        )),
        ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_1 => Some(Wrapping::AesKeyWrap(
            &::aws_lc_rs::rsa::OAEP_SHA1_MGF1SHA1,
        )),
        ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_256 => Some(Wrapping::AesKeyWrap(
            &::aws_lc_rs::rsa::OAEP_SHA256_MGF1SHA256,
        )),
        _ => None,
    }
}

/// The `AlgorithmSpec` values a caller wraps key material with directly under the RSA
/// public key — see [`wrapping_of`].
pub const ALGORITHM_SPEC_RSAES_OAEP_SHA_1: &str = "RSAES_OAEP_SHA_1";
/// See [`ALGORITHM_SPEC_RSAES_OAEP_SHA_1`].
pub const ALGORITHM_SPEC_RSAES_OAEP_SHA_256: &str = "RSAES_OAEP_SHA_256";
/// The `AlgorithmSpec` values that wrap key material with an ephemeral AES key and that
/// AES key under the RSA public key — see [`wrapping_of`]. They are what "supported for
/// wrapping RSA and ECC key material" names
/// (`smithy_query(query="doc",shape="GetParametersForImportRequest",member="WrappingAlgorithm")`).
pub const ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_1: &str = "RSA_AES_KEY_WRAP_SHA_1";
/// See [`ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_1`].
pub const ALGORITHM_SPEC_RSA_AES_KEY_WRAP_SHA_256: &str = "RSA_AES_KEY_WRAP_SHA_256";

/// How long one wrapping key pair and its import token stay usable: "Each public key
/// and import token set is valid for 24 hours"
/// (`smithy_query(query="doc",shape="GetParametersForImport")`).
pub const IMPORT_PARAMETERS_VALIDITY: std::time::Duration =
    std::time::Duration::from_secs(24 * 60 * 60);

/// The largest number of outstanding wrapping key pairs this store holds.
///
/// A bound the EMULATOR imposes, not a quota KMS publishes: `GetParametersForImport`
/// mints a key pair per call and is reachable by any caller, so without a cap a loop
/// of valid calls grows the process without end. Past it the operation refuses with a
/// modelled server error and mints nothing.
pub const MAX_IMPORT_PARAMETERS: usize = 200;

/// One wrapping key pair `GetParametersForImport` handed out, with the import token
/// that names it.
///
/// "The public key (or 'wrapping key') of an RSA key pair that KMS generates" is
/// returned to the caller and the private half stays here, which is what lets
/// `ImportKeyMaterial` open the key material the caller wraps with it. "The public key
/// and its import token are permanently linked and must be used together"
/// (`smithy_query(query="doc",shape="GetParametersForImport")`), so the token is the
/// key of this record and the record holds everything the import must agree with: the
/// KMS key the parameters were issued for, and the wrapping algorithm the caller said
/// it would use.
///
/// `Debug` is written by hand rather than derived so the private key is REDACTED.
#[derive(Clone)]
pub struct ImportParameters {
    /// The key id of the KMS key the parameters were issued for.
    pub key_id: String,
    /// The `AlgorithmSpec` the request named, which is the algorithm the caller
    /// wraps the key material with.
    pub wrapping_algorithm: String,
    /// The `WrappingKeySpec` the request named, which is the kind of RSA key pair
    /// that was generated.
    pub wrapping_key_spec: String,
    /// The PKCS#8 DER of the private half of the wrapping key pair. It never leaves
    /// this process: only the public half is returned, and only `ImportKeyMaterial`
    /// reads this one.
    pub private_key_pkcs8: Vec<u8>,
    /// When the pair and its token stop being usable, which is
    /// [`IMPORT_PARAMETERS_VALIDITY`] after the call that minted them.
    pub valid_to: SystemTime,
}

impl std::fmt::Debug for ImportParameters {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ImportParameters")
            .field("key_id", &self.key_id)
            .field("wrapping_algorithm", &self.wrapping_algorithm)
            .field("wrapping_key_spec", &self.wrapping_key_spec)
            .field("private_key_pkcs8", &"<redacted>")
            .field("valid_to", &self.valid_to)
            .finish()
    }
}

impl ImportParameters {
    /// Whether these parameters are still usable at `now`: an import token past its
    /// `ParametersValidTo` "cannot be used in an ImportKeyMaterial request"
    /// (`smithy_query(query="doc",shape="GetParametersForImport")`).
    pub fn usable_at(&self, now: SystemTime) -> bool {
        now < self.valid_to
    }
}

/// The import token that names one [`ImportParameters`] record: opaque to the caller,
/// drawn from the OS CSPRNG so no two calls share one and no client can construct one.
///
/// `None` means the platform refused to produce entropy; the caller answers a modelled
/// server error rather than minting a predictable token.
pub fn mint_import_token() -> Option<Vec<u8>> {
    random_bytes(IMPORT_TOKEN_LEN)
}

/// How many bytes an import token carries. The value is this emulator's: `ImportToken`
/// is only "the import token to send in a subsequent ImportKeyMaterial request"
/// (`smithy_query(query="doc",shape="GetParametersForImportResponse",member="ImportToken")`)
/// with a modelled 1..6144 length and no structure.
const IMPORT_TOKEN_LEN: usize = 32;

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
