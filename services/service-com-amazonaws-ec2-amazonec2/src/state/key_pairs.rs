// SPDX-License-Identifier: CC0-1.0

//! The account's KEY PAIRS: the record one is stored as, the bounds a create holds a
//! request to, the refusals EC2 answers about one, and the one writer that stores one.
//!
//! EC2 keeps only the PUBLIC half: "Amazon EC2 stores the public key and displays the
//! private key for you to save to a file" (`CreateKeyPair`), and a delete removes a key
//! pair "by removing the public key from Amazon EC2" (`DeleteKeyPair`). So a stored
//! [`KeyPairRecord`] holds the public key, the fingerprint and everything else
//! [`KeyPairInfo`] reports — and never the private key, which is handed to the caller that
//! asked for it and kept nowhere. That is also why a record carries no key material a
//! later read could disclose.
//!
//! The private half itself, and the file formats the two `KeyFormat` members name, are
//! [`super::key_pair_material`]'s subject; this file is the STORE.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

use crate::ops::types::KeyPairInfo;

/// The prefix of every key pair id, as [`ComAmazonawsEc2Amazonec2State::next_id`] takes it:
/// a key pair id is `key-` and 17 lowercase hexadecimal digits, the long form every EC2
/// resource id this store mints takes.
pub const KEY_PAIR_ID_PREFIX: &str = "key";

/// The `ResourceType` a `CreateKeyPair` tag specification may name: the key pair it creates
/// (the modelled `ResourceType` enum's `key-pair` member).
pub const RESOURCE_TYPE_KEY_PAIR: &str = "key-pair";

/// "Up to 255 ASCII characters" (`CreateKeyPairRequest$KeyName`).
pub const MAX_KEY_PAIR_NAME_CHARS: usize = 255;

/// Refuse a key pair name outside what EC2 publishes for one: "a unique name for the key
/// pair. Constraints: up to 255 ASCII characters" (`CreateKeyPairRequest$KeyName`).
///
/// The ONE reading of that sentence, so the two calls that take a key pair name from a
/// caller — `CreateKeyPair` and `ImportKeyPair` — hold it to one rule in one wording rather
/// than two that can drift. The three clauses are reported apart, because a name the request
/// left empty, one longer than the maximum and one carrying a character outside ASCII are
/// different mistakes: the member is modelled as a plain `String`, so the boundary validator
/// enforces none of them.
pub fn validate_key_pair_name<E>(key_name: &str) -> Result<(), Scaffolded<E>> {
    if key_name.is_empty() {
        return Err(Scaffolded::missing_parameter(
            "The request must contain the parameter KeyName",
        ));
    }
    if key_name.chars().count() > MAX_KEY_PAIR_NAME_CHARS {
        return Err(invalid_parameter_value(format!(
            "Value ({key_name}) for parameter keyName is invalid. \
             A key pair name accepts at most {MAX_KEY_PAIR_NAME_CHARS} characters"
        )));
    }
    if !key_name.is_ascii() {
        return Err(invalid_parameter_value(format!(
            "Value ({key_name}) for parameter keyName is invalid. \
             A key pair name accepts ASCII characters only"
        )));
    }
    Ok(())
}

/// The largest number of key pairs this store holds.
///
/// "You can have up to 5,000 key pairs per Amazon Web Services Region" (`CreateKeyPair`).
/// [`ComAmazonawsEc2Amazonec2State::has_key_pair_capacity`] is the predicate a writer asks
/// before inserting; a create past it refuses with [`key_pair_limit_exceeded`].
pub const MAX_KEY_PAIRS: usize = 5_000;

/// The code EC2 answers for a key pair name the account already holds a key pair under:
/// "the specified key pair name already exists" (AWS's `InvalidKeyPair.Duplicate`
/// error-code entry).
pub const INVALID_KEY_PAIR_DUPLICATE: &str = "InvalidKeyPair.Duplicate";

/// The code EC2 answers for a key pair a request named that the account holds none of:
/// "the specified key pair name does not exist" (AWS's `InvalidKeyPair.NotFound`
/// error-code entry).
pub const INVALID_KEY_PAIR_NOT_FOUND: &str = "InvalidKeyPair.NotFound";

/// The code a create past [`MAX_KEY_PAIRS`] answers with: "you've reached the limit on the
/// number of key pairs you can have" (AWS's `KeyPairLimitExceeded` error-code entry).
pub const KEY_PAIR_LIMIT_EXCEEDED: &str = "KeyPairLimitExceeded";

/// Refuse a second key pair with a name the account already holds: HTTP 400
/// [`INVALID_KEY_PAIR_DUPLICATE`]. "If a key with the specified name already exists, Amazon
/// EC2 returns an error" (`CreateKeyPair`), the name being "a unique name for the key pair"
/// (`CreateKeyPairRequest$KeyName`).
///
/// The message is EC2's whole sentence for this error class, which carries no variable part:
/// the recorded response to a duplicate create words it exactly this way, whatever name the
/// request named, so the name is not interpolated into it.
pub fn invalid_key_pair_duplicate<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_KEY_PAIR_DUPLICATE,
        400,
        DUPLICATE_KEY_PAIR_MESSAGE,
    )
}

/// EC2's sentence for a create whose key pair name the account already holds.
pub const DUPLICATE_KEY_PAIR_MESSAGE: &str = "The keypair already exists";

/// Refuse a request naming a key pair NAME the account holds no key pair under: HTTP 400
/// [`INVALID_KEY_PAIR_NOT_FOUND`].
pub fn invalid_key_pair_name_not_found<E>(key_name: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_KEY_PAIR_NOT_FOUND,
        400,
        format!("The key pair '{key_name}' does not exist"),
    )
}

/// Refuse a request naming a key pair ID the account holds no key pair under: HTTP 400
/// [`INVALID_KEY_PAIR_NOT_FOUND`] — the same code as
/// [`invalid_key_pair_name_not_found`], which is the one EC2 answers for either selector,
/// naming what the request actually gave it.
pub fn invalid_key_pair_id_not_found<E>(key_pair_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_KEY_PAIR_NOT_FOUND,
        400,
        format!("The key pair ID '{key_pair_id}' does not exist"),
    )
}

/// Refuse a create that would take the store past [`MAX_KEY_PAIRS`]: HTTP 400
/// [`KEY_PAIR_LIMIT_EXCEEDED`].
pub fn key_pair_limit_exceeded<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        KEY_PAIR_LIMIT_EXCEEDED,
        400,
        format!("The maximum number of key pairs has been reached ({MAX_KEY_PAIRS})"),
    )
}

/// One stored key pair: the modelled [`KeyPairInfo`] shape every report of a key pair
/// answers with, beside the one fact that shape carries only on request.
///
/// `PublicKey` is "included in the response" only "if `IncludePublicKey` is true"
/// (`DescribeKeyPairsRequest$IncludePublicKey`), so the public key material is held BESIDE
/// the reported view rather than inside it: a read that did not ask for it cannot report it
/// by accident, and [`Self::view`] is the one place the choice is made.
#[derive(Debug, Clone)]
pub struct KeyPairRecord {
    /// Everything `DescribeKeyPairs` reports about this key pair except its public key
    /// material: its id, name, type, fingerprint, creation time and tags.
    pub info: KeyPairInfo,
    /// "The public key material" (`KeyPairInfo$PublicKey`), as the single-line OpenSSH
    /// public key the key's own algorithm names it with — the public half EC2 stores.
    pub public_key: String,
}

impl KeyPairRecord {
    /// This key pair as `DescribeKeyPairs` reports it, carrying the public key material
    /// only when the request asked for it.
    pub fn view(&self, include_public_key: bool) -> KeyPairInfo {
        let mut info = self.info.clone();
        info.public_key = include_public_key.then(|| self.public_key.clone());
        info
    }
}

/// What a writer must say to store a key pair; everything else about the record — the id,
/// the creation time's rendering — is derived by
/// [`ComAmazonawsEc2Amazonec2State::insert_key_pair`].
#[derive(Debug, Clone)]
pub struct KeyPairSpec {
    /// "A unique name for the key pair" (`CreateKeyPairRequest$KeyName`).
    pub key_name: String,
    /// "The type of key pair" (`CreateKeyPairRequest$KeyType`), as a `&'static str` out of
    /// the modelled `KeyType` catalog, so the stored word is never the request's own text.
    pub key_type: &'static str,
    /// The fingerprint of the key material that was generated, as the key's own type
    /// fingerprints it ([`super::key_pair_material`]).
    pub key_fingerprint: String,
    /// The public half EC2 stores, as the OpenSSH public key line.
    pub public_key: String,
    /// The tags the request's `TagSpecifications` assign to the key pair.
    pub tags: Vec<Tag>,
    /// "The date and time when the key was created" (`KeyPairInfo$CreateTime`).
    pub created_at: SystemTime,
}

/// The caller-chosen bytes one stored key pair holds, plus the fixed overhead the record
/// itself costs.
///
/// The name and the tags are the request's, and the public key is as long as the key TYPE
/// the request chose makes it, so all three are charged on top of
/// [`STORED_RECORD_OVERHEAD_BYTES`]. The ONE definition of that charge, the way
/// [`placement_group_spec_bytes`] is for a placement group: `CreateKeyPair` reserves exactly
/// it and [`ComAmazonawsEc2Amazonec2State::insert_key_pair`] charges exactly it, so the
/// budget a create checks cannot drift from what the insert takes.
pub fn key_pair_spec_bytes(spec: &KeyPairSpec) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(spec.key_name.len())
        .saturating_add(spec.public_key.len())
        .saturating_add(tag_list_bytes(&spec.tags))
}

/// The same charge, read off a key pair AS IT IS STORED, so a delete gives back exactly
/// what the insert took.
pub fn key_pair_record_bytes(record: &KeyPairRecord) -> usize {
    STORED_RECORD_OVERHEAD_BYTES
        .saturating_add(record.info.key_name.as_deref().unwrap_or_default().len())
        .saturating_add(record.public_key.len())
        .saturating_add(tag_list_bytes(record.info.tags.as_deref().unwrap_or_default()))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another key pair fits inside the store's ceiling ([`MAX_KEY_PAIRS`]).
    ///
    /// The same contract as [`Self::has_placement_group_capacity`]: a create asks first and
    /// refuses with [`key_pair_limit_exceeded`] when it answers `false`.
    pub fn has_key_pair_capacity(&self) -> bool {
        self.key_pairs.len() < MAX_KEY_PAIRS
    }

    /// The id of the key pair named `key_name`, if the account holds one.
    ///
    /// A name is "a unique name for the key pair" (`CreateKeyPairRequest$KeyName`), so this
    /// answers at most one: it is both what a create asks before storing a duplicate and
    /// how a request that names a key pair by name resolves it. Compared byte for byte, the
    /// name being an opaque identifier the model publishes no case rule for.
    pub fn key_pair_id_for_name(&self, key_name: &str) -> Option<String> {
        self.key_pairs
            .iter()
            .find(|(_, record)| record.info.key_name.as_deref() == Some(key_name))
            .map(|(key_pair_id, _)| key_pair_id.clone())
    }

    /// Store a key pair and answer it as it was stored — or `None` when the store is at
    /// [`MAX_KEY_PAIRS`] and nothing was stored.
    ///
    /// The one way a key pair is stored, so the id it carries and the bytes it charges
    /// cannot be built two ways, and the ceiling is asked HERE as well as by the caller.
    /// What comes back is the stored record itself, so `CreateKeyPair` answers exactly what
    /// a later `DescribeKeyPairs` reports without reading the store a second time.
    pub fn insert_key_pair(&mut self, spec: KeyPairSpec) -> Option<KeyPairRecord> {
        if !self.has_key_pair_capacity() {
            return None;
        }
        let key_pair_id = self.next_id(KEY_PAIR_ID_PREFIX);
        // Charged from the spec, so the quantity is exactly the one `CreateKeyPair`
        // reserved against the byte budget before calling this.
        self.add_stored_bytes(key_pair_spec_bytes(&spec));
        let record = KeyPairRecord {
            info: KeyPairInfo {
                key_pair_id: Some(key_pair_id.clone()),
                key_name: Some(spec.key_name),
                key_type: Some(spec.key_type.to_string()),
                key_fingerprint: Some(spec.key_fingerprint),
                create_time: Some(system_time_to_date_time(spec.created_at)),
                tags: Some(spec.tags),
                // Held beside the view, not in it: a read reports it only when it asked
                // for it ([`KeyPairRecord::view`]).
                public_key: None,
            },
            public_key: spec.public_key,
        };
        let stored = record.clone();
        self.key_pairs.insert(key_pair_id, record);
        Some(stored)
    }

    /// Take the key pair `key_pair_id` names out of the store and answer it as it was
    /// stored, or `None` when the account holds no key pair under that id.
    ///
    /// The one way a key pair STOPS being stored, the counterpart of
    /// [`Self::insert_key_pair`]: it gives the record's caller-chosen bytes back to the
    /// store's budget ([`key_pair_record_bytes`]), so a create-delete-create cycle cannot
    /// leak budget, and a later read resolves neither the id nor the name any more.
    pub fn remove_key_pair(&mut self, key_pair_id: &str) -> Option<KeyPairRecord> {
        let record = self.key_pairs.remove(key_pair_id)?;
        self.release_stored_bytes(key_pair_record_bytes(&record));
        Some(record)
    }
}
