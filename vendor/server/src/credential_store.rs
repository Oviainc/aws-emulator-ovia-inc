// Copyright Amazon.com, Inc. or its affiliates. All Rights Reserved.
// SPDX-License-Identifier: Apache-2.0
//! The credentials an emulator accepts, whatever their scheme: the key it publishes at start, and
//! every credential its operations create. A [`CredentialKind`] mints each kind, so the
//! emulator generates every id and secret and never hands a secret back after creating it. Each
//! emulator holds its own store, in memory only.

use crate::request_authentication::{ACCESS_DENIED, Denied};
use std::collections::{BTreeMap, HashMap};

/// A credential's secret material, by name: what a request is checked against.
pub type Secrets = BTreeMap<&'static str, String>;

/// One kind of credential: the scheme its requests are verified under, and how a fresh id with
/// its secrets is minted. Sealed: only the emulator defines kinds, so no operation chooses an id
/// or a secret.
pub trait CredentialKind: Send + Sync + sealed::Sealed {
    /// The Smithy auth scheme shape id requests signed with it present (`aws.auth#sigv4`).
    fn scheme(&self) -> &'static str;
    fn mint(&self) -> MintedCredential;
}

/// The credentials one emulator's operations created, keyed by the id a request presents. The
/// key the emulator published at start is not among them.
#[derive(Default)]
pub struct CredentialStore {
    records: parking_lot::RwLock<HashMap<String, CredentialRecord>>,
}

impl CredentialStore {
    /// Creates a fresh credential of `kind`; its secrets are never readable again. It never
    /// replaces a credential the store holds: an id already in use is minted again.
    pub fn create(&self, kind: &'static dyn CredentialKind) -> MintedCredential {
        let mut records = self.records.write();
        let mut minted = kind.mint();
        while records.contains_key(&minted.id) {
            minted = kind.mint();
        }
        let (scheme, secrets) = (kind.scheme(), minted.secrets.clone());
        records.insert(minted.id.clone(), CredentialRecord { scheme, secrets });
        minted
    }

    /// Forgets a credential, so requests presenting it are refused.
    pub fn delete(&self, id: &str) -> Result<(), CredentialError> {
        let removed = self.records.write().remove(id);
        removed.map(drop).ok_or(CredentialError::NotFound)
    }

    /// The secret named `name` of the credential of `scheme` with this id.
    pub(crate) fn secret_for(&self, id: &str, scheme: &str, name: &str) -> Result<String, Denied> {
        let records = self.records.read();
        let record = records.get(id).filter(|record| record.scheme == scheme);
        let secret = record.and_then(|record| record.secrets.get(name));
        secret.cloned().ok_or(ACCESS_DENIED)
    }
}

/// A minted credential, the only value that ever carries its secrets.
#[derive(Clone)]
pub struct MintedCredential {
    pub id: String,
    pub secrets: Secrets,
}

impl MintedCredential {
    pub fn secret(&self, name: &str) -> Option<&str> {
        self.secrets.get(name).map(String::as_str)
    }
}

/// Prints the id but never a secret, so a logged value leaks nothing.
impl std::fmt::Debug for MintedCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MintedCredential")
            .field("id", &self.id)
            .finish_non_exhaustive()
    }
}

/// Why the store refused to delete a credential.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CredentialError {
    NotFound,
}

struct CredentialRecord {
    scheme: &'static str,
    secrets: Secrets,
}

pub(crate) mod sealed {
    pub trait Sealed {}
}

pub(crate) fn same_secret(expected: &str, provided: &str) -> bool {
    aws_lc_rs::constant_time::verify_slices_are_equal(expected.as_bytes(), provided.as_bytes())
        .is_ok()
}

/// `length` characters drawn uniformly from `alphabet`, whose length divides 256.
pub(crate) fn random_from(alphabet: &[u8], length: usize) -> String {
    let mut random = vec![0u8; length];
    aws_lc_rs::rand::fill(&mut random).expect("the system random source is available");
    let pick = |byte: &u8| char::from(alphabet[usize::from(*byte) % alphabet.len()]);
    random.iter().map(pick).collect()
}

#[cfg(test)]
#[path = "credential_store_tests.rs"]
mod tests;
