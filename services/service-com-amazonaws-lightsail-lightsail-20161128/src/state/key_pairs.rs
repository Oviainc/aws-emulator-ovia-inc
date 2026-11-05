// SPDX-License-Identifier: CC0-1.0

//! The stored SSH key pair: the public half this service holds for the account,
//! under the name an instance's key pair member names it by.

use super::*;

/// One stored SSH key pair: the PUBLIC half Lightsail holds for the account,
/// under the name an instance's `sshKeyName` names it by.
///
/// Lightsail holds only the public half — the modelled `DeleteKeyPair`
/// documentation deletes a key pair "by removing the public key from Amazon
/// Lightsail" — so that is all a record here holds beside what the service
/// assigned it. The private half is handed to the caller by the call that
/// created the pair and is never stored.
#[derive(Debug, Clone)]
pub struct StoredKeyPair {
    /// The key pair name the request named, kept verbatim — it is this record's
    /// store key and the `name` every read reports.
    pub name: String,
    pub arn: String,
    pub support_code: String,
    pub created_at_ms: i64,
    pub location: StoredLocation,
    pub tags: Vec<Tag>,
    /// The fingerprint of the public half: "The RSA fingerprint of the key pair"
    /// (`KeyPair$fingerprint`). It is what a delete's `expectedFingerprint`
    /// states it expects to be deleting.
    pub fingerprint: String,
}

impl StoredKeyPair {
    /// This record as the modelled `KeyPair`.
    pub fn view(&self) -> KeyPair {
        KeyPair {
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            support_code: Some(self.support_code.clone()),
            created_at: Some(timestamp(self.created_at_ms)),
            location: Some(self.location.view()),
            resource_type: Some(RESOURCE_TYPE_KEY_PAIR.to_string()),
            tags: Some(self.tags.clone()),
            fingerprint: Some(self.fingerprint.clone()),
        }
    }

    /// Whether this is the account's Lightsail DEFAULT key pair — the one
    /// [`DEFAULT_SSH_KEY_NAME`] names, which an instance created without a key
    /// pair of its own is launched with.
    ///
    /// The record's NAME is what decides, because that name is how every
    /// Lightsail operation addresses the default pair.
    pub fn is_default(&self) -> bool {
        self.name == DEFAULT_SSH_KEY_NAME
    }
}
