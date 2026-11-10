// SPDX-License-Identifier: CC0-1.0

//! Whether KMS will let this service USE the customer managed key a vector bucket or a
//! vector index is to be encrypted under.
//!
//! An `encryptionConfiguration` naming `aws:kms` with a `kmsKeyArn` is not a flag this
//! service only stores and reports back: a vector bucket or vector index created under
//! such a key is created THROUGH that key, so the creating call uses it on the caller's
//! behalf before anything exists. A key KMS will not use — one a customer has disabled,
//! or one whose deletion is scheduled — therefore refuses the create with
//! `AccessDeniedException` (HTTP 403) and leaves nothing behind, so a later read of the
//! name the refused call used still answers `NotFoundException`; making the key usable
//! again makes the identical call succeed. That is what makes a customer's hold on the
//! key an effective control over the vector store encrypted under it.
//!
//! Two sentences, because two different uses of the key are being refused. A request that
//! NAMES the key itself reaches KMS as the caller, so the caller is told what KMS said
//! about the key it named — KMS's own key-state sentence
//! ([`kms_store::invalid_state_message`]). A vector index that INHERITS its bucket's key
//! names none, and what cannot be performed is the asynchronous indexing this service does
//! for the index under its own service principal, so the sentence is about that access
//! instead ([`insufficient_indexing_access_message`]). Either way the message closes with
//! the id of the KMS call made on the caller's behalf, so the refused attempt can be found
//! in KMS's own records.
//!
//! The key is resolved in the KMS service's OWN store on the shared engine, so this reads
//! exactly what that service's `CreateKey`, `DisableKey`, `EnableKey` and
//! `ScheduleKeyDeletion` handlers wrote and the two services cannot disagree about a key.
//! Nothing is withheld on evidence that is not there: an engine carrying no KMS store
//! beside this service, an identifier that store holds no key for, and a key AWS itself
//! manages (whose key state no customer moves) all leave the create to be served.
//!
//! What this is NOT: a reading of the key's own policy documents. The key state is the one
//! fact about a key this service is known to turn a create on, and it is the whole of the
//! decision here, so a key a customer holds usable admits the creates its vector store
//! needs without a grant written for this service.
//!
//! LOCKING: this takes the KMS store's guard, and a handler may ask it while holding this
//! service's own. No KMS operation reads this service's store, so the two cannot deadlock,
//! and the work done under both guards is a single map lookup.

use ::service_com_amazonaws_kms_trentservice::state as kms_store;

use crate::ops::types::EncryptionConfiguration;

/// The service principal this service performs a vector index's asynchronous indexing
/// under, which is the principal whose access to the key an inherited-key refusal is
/// about.
const INDEXING_SERVICE_PRINCIPAL: &str = "indexing.s3vectors.amazonaws.com";

/// Where the customer managed key a create would encrypt under came from, which is what
/// decides how its refusal is worded (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    /// The request's own `encryptionConfiguration` named the key.
    Named,
    /// The request named no key, so the key in force is the one the resource inherits —
    /// its parent vector bucket's, for a vector index.
    Inherited,
}

/// [`KeySource`] for one create request: the request named the key exactly when its own
/// `encryptionConfiguration` carries a `kmsKeyArn`, whatever operation it is a request to,
/// and otherwise the key in force is one the resource inherits.
pub fn key_source(requested: Option<&EncryptionConfiguration>) -> KeySource {
    if requested.is_some_and(|requested| requested.kms_key_arn.is_some()) {
        KeySource::Named
    } else {
        KeySource::Inherited
    }
}

/// Why KMS refuses this service's use of the key `encryption` names, or `None` when the
/// create may be served.
///
/// `region` is the Region the request was signed for, which is where the key is resolved: a
/// vector store and the key it is encrypted under answer at the same endpoint.
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    encryption: &super::EncryptionRecord,
    source: KeySource,
) -> Option<String> {
    if encryption.sse_type != super::KMS_SSE_TYPE {
        return None;
    }
    let identifier = encryption.kms_key_arn.as_deref()?;
    let kms = kms_store::service_state(engine)?;
    let store = kms.lock();
    let kms_store::KeyRef::Found(key) = store.resolve(region, identifier) else {
        return None;
    };
    if key.key_manager != kms_store::KEY_MANAGER_CUSTOMER || key.is_usable() {
        return None;
    }
    let sentence = match source {
        KeySource::Inherited => insufficient_indexing_access_message(),
        KeySource::Named => kms_store::invalid_state_message(key),
    };
    Some(with_kms_request_id(&sentence))
}

/// How a create whose key the resource INHERITS is refused: the asynchronous indexing this
/// service performs for the index cannot reach the key, and the sentence says what about
/// the key has to be true for it to — that it exists, that it is active, and that its own
/// policy admits the principal the indexing runs as ([`INDEXING_SERVICE_PRINCIPAL`]).
fn insufficient_indexing_access_message() -> String {
    format!(
        "Insufficient access to perform asynchronous indexing. Ensure that the key used to \
         encrypt the index still exists, is active, and has a resource policy granting access \
         to the S3 service principal '{INDEXING_SERVICE_PRINCIPAL}'."
    )
}

/// Close a refusal with the id of the KMS call this service made on the caller's behalf.
///
/// One id per call — a correlation id for THAT attempt to use the key, so it is minted
/// fresh ([`::service_common::query_error_envelope::new_request_id`]) and never derived
/// from the request or stored.
fn with_kms_request_id(sentence: &str) -> String {
    format!(
        "{sentence} (kms_request_id={})",
        ::service_common::query_error_envelope::new_request_id()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Which key a create uses is read off the request alone: a configuration carrying a
    /// key names it, and everything else leaves the resource inheriting one. The rows
    /// differ along what the request's own configuration holds.
    #[test]
    fn a_request_names_its_key_only_when_it_carries_one() {
        let rows: [(&str, Option<EncryptionConfiguration>, KeySource); 4] = [
            ("no configuration at all", None, KeySource::Inherited),
            (
                "the KMS type with a key",
                Some(EncryptionConfiguration {
                    sse_type: super::super::KMS_SSE_TYPE.to_string(),
                    kms_key_arn: Some("arn:aws:kms:us-east-1:000000000000:key/k".to_string()),
                }),
                KeySource::Named,
            ),
            (
                "a configuration naming only the S3 managed type",
                Some(EncryptionConfiguration {
                    sse_type: super::super::DEFAULT_SSE_TYPE.to_string(),
                    kms_key_arn: None,
                }),
                KeySource::Inherited,
            ),
            (
                "an empty configuration",
                Some(EncryptionConfiguration {
                    sse_type: String::new(),
                    kms_key_arn: None,
                }),
                KeySource::Inherited,
            ),
        ];
        for (case, requested, expected) in rows {
            let actual = key_source(requested.as_ref());
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// Every refusal this module words closes with the id of the KMS call it reports, and
    /// two refusals carry two ids.
    #[test]
    fn a_refusal_closes_with_the_id_of_the_kms_call() {
        let sentence = insufficient_indexing_access_message();
        let first = with_kms_request_id(&sentence);
        let second = with_kms_request_id(&sentence);
        for (case, message) in [("first", &first), ("second", &second)] {
            let suffix = message
                .strip_prefix(sentence.as_str())
                .unwrap_or_else(|| panic!("case {case:?}: expected the sentence, got {message:?}"));
            let id = suffix
                .strip_prefix(" (kms_request_id=")
                .and_then(|rest| rest.strip_suffix(')'))
                .unwrap_or_else(|| panic!("case {case:?}: expected the suffix, got {suffix:?}"));
            let groups: Vec<usize> = id.split('-').map(str::len).collect();
            assert_eq!(
                groups,
                vec![8, 4, 4, 4, 12],
                "case {case:?}: expected a canonical UUID, got {id:?}"
            );
        }
        assert_ne!(
            first, second,
            "case two calls: each call reports its own id, got {first:?} twice"
        );
    }
}
