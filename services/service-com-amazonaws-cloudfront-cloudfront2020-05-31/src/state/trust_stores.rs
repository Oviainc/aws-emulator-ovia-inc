// SPDX-License-Identifier: CC0-1.0

//! The TRUST STORE slice of this service's state.
//!
//! "A trust store" is its own resource kind — addressed by a minted `Id`, carrying the CA
//! certificates bundle it was created from and the number of certificates that bundle
//! holds — so everything about how one is persisted and projected lives here rather than
//! in the parent module: the record, the id and ARN one is addressed by, the lookup every
//! read goes through, and the modelled errors the trust store operations answer with. The
//! parent [`ComAmazonawsCloudfrontCloudfront20200531State`] holds the one map
//! (`trust_stores`) and this module holds everything that reads or writes it.
//!
//! `NumberOfCaCertificates` is "The trust store's number of CA certificates", so it is
//! never a value this emulator picks: it is the count of certificates READ out of the
//! bundle the create named, in the one object store this emulator holds
//! ([`super::ca_certificates_bundle`]). A bundle that store answers is unreadable is a
//! create that is refused, so every trust store in here was made from a bundle that was
//! actually read.
//!
//! One thing is owed by the CLOCK: a trust store CloudFront has just created reports
//! [`TRUST_STORE_STATUS_PENDING`] until its validated bundle has propagated to the edge
//! locations, and [`TRUST_STORE_STATUS_ACTIVE`] after
//! ([`super::PROPAGATION_DELAY`], the span every other propagation of this service is
//! owed on). [`StoredTrustStore::settle`] applies it, and nothing ever sleeps for it.

use super::*;
use crate::ops::types::{CaCertificatesBundleSource, TrustStore};

/// The prefix every trust store id this service mints carries.
///
/// `GetTrustStore`'s `Identifier` is "The trust store's identifier", which — like every
/// other CloudFront resource addressed by an `Identifier` — resolves an id, a name or an
/// ARN, so the three namespaces have to be told apart by shape: an ARN starts with `arn:`
/// and a name is what a caller chose, so a minted id carries a namespace label of its own,
/// exactly as a distribution tenant's ([`DISTRIBUTION_TENANT_ID_PREFIX`]) and a connection
/// group's ([`CONNECTION_GROUP_ID_PREFIX`]) do. The model states no id pattern, so the
/// prefix is this emulator's choice.
pub const TRUST_STORE_ID_PREFIX: &str = "ts_";

/// `TrustStore$Status` while the trust store's CA certificates bundle is still propagating
/// to the edge locations (`smithy_query(query="shape",shape="TrustStoreStatus")`, whose
/// values are `pending`, `active` and `failed`).
///
/// A trust store CloudFront has just accepted is not serving viewer requests yet, so the
/// create answers this non-terminal status and the clock — not a later call — owes the
/// trust store [`TRUST_STORE_STATUS_ACTIVE`].
pub const TRUST_STORE_STATUS_PENDING: &str = "pending";

/// `TrustStore$Status` once the bundle has propagated: the terminal status a trust store
/// validates client certificates in.
pub const TRUST_STORE_STATUS_ACTIVE: &str = "active";

/// `CreateTrustStore$UseClientCertificateOCSPEndpoint` for a create that named none.
///
/// "A Boolean that determines whether to use the CA certificate's OCSP endpoint to check
/// certificate revocation status": the member is optional, and checking revocation against
/// the certificate's own OCSP responder is a behaviour a caller asks for, so leaving it out
/// asks for the trust store that does not.
pub const DEFAULT_USE_CLIENT_CERTIFICATE_OCSP_ENDPOINT: bool = false;

/// The most trust stores this emulator holds.
///
/// `CreateTrustStore` models `EntityLimitExceeded` ("The entity limit has been exceeded"),
/// so the store is bounded; the bound is this emulator's, CHOSEN like
/// [`MAX_KEY_VALUE_STORES`] rather than read off a published quota, and it is an entry
/// count refused before anything is minted.
pub const MAX_TRUST_STORES: usize = 1_000;

/// The ARN of a trust store. CloudFront is a GLOBAL service, so the ARN's region segment is
/// empty: `arn:aws:cloudfront::<account>:trust-store/<Id>`, the resource type this
/// service's own `/trust-store` routes name.
pub fn trust_store_arn(id: &str) -> String {
    ::engine::emulator_arn("cloudfront", "", &format!("trust-store/{id}"))
}

/// The sentence CloudFront answers an identifier no trust store answers to with.
///
/// It states the CONDITION and echoes no part of the request, so the same sentence answers
/// an unknown id, an unknown name and an identifier of no form this service issues — which
/// is the recorded contract of the trust store reads.
pub const TRUST_STORE_NOT_FOUND_MESSAGE: &str = "The specified trust store does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for an identifier — an id, a name or an ARN —
/// no stored trust store answers to.
///
/// Takes no identifier: the sentence is fixed ([`TRUST_STORE_NOT_FOUND_MESSAGE`]), so every
/// operation that resolves a trust store refuses this condition the same way.
pub fn trust_store_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(TRUST_STORE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers a trust store name that is already taken with, on the
/// wire: the text for `EntityAlreadyExists` on THIS resource kind, worded as
/// [`CONNECTION_GROUP_ALREADY_EXISTS_MESSAGE`] words it for a neighbouring kind, and fixed
/// whole for the reason that one is — the sentence the MODEL's documentation of the shape
/// uses ("The entity already exists. You must provide a unique entity.") documents the
/// condition for every kind sharing the code and is not served, and nothing from the
/// request or from the store belongs in the served text.
pub const TRUST_STORE_ALREADY_EXISTS_MESSAGE: &str =
    "A trust store with the same name already exists in this AWS account.";

/// The modelled `EntityAlreadyExists` (HTTP 409) for a trust store name that is already
/// taken.
///
/// Takes no name, for the reason [`connection_group_already_exists`] takes none: every
/// refusal of this condition reads the same
/// ([`TRUST_STORE_ALREADY_EXISTS_MESSAGE`]).
pub fn trust_store_already_exists() -> EntityAlreadyExists {
    EntityAlreadyExists {
        message: Some(TRUST_STORE_ALREADY_EXISTS_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers a CA certificates bundle it cannot use with, on the
/// wire: the recorded `InvalidArgument` text for THIS condition, and the whole sentence is
/// fixed — what the refusal states is that the bundle the request carried is not one a trust
/// store can be built from, which names neither the bucket, nor the key, nor the way the
/// bytes fall short.
pub const INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE: &str =
    "The request contains an invalid CA Certificates Bundle.";

/// The modelled `InvalidArgument` (HTTP 400) for a CA certificates bundle source that
/// addresses no object at all, or whose object holds no CA certificate
/// ([`INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE`]).
pub fn invalid_ca_certificates_bundle() -> crate::ops::types::InvalidArgument {
    crate::ops::types::InvalidArgument {
        message: Some(INVALID_CA_CERTIFICATES_BUNDLE_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront answers a CA certificates bundle the object store does not hold
/// with: the not-found text for THIS entity, worded as
/// [`TRUST_STORE_NOT_FOUND_MESSAGE`] words it for the trust store itself, and fixed whole
/// for the same reason — the condition is that the address names no object, so neither the
/// bucket nor the key is part of it.
pub const CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE: &str =
    "The specified CA Certificates Bundle does not exist.";

/// The modelled `EntityNotFound` (HTTP 404) for a CA certificates bundle address the object
/// store answers that it holds nothing at — an absent bucket, object or version.
///
/// The ENTITY that was not found is the bundle, not the trust store: the trust store is what
/// the refused call would have created.
pub fn ca_certificates_bundle_not_found() -> EntityNotFound {
    EntityNotFound {
        message: Some(CA_CERTIFICATES_BUNDLE_NOT_FOUND_MESSAGE.to_string()),
    }
}

/// The message a create answers when nothing in this emulator answered the read of its CA
/// certificates bundle: there is no object store to ask, or the object's bytes never
/// arrived, so the operation cannot be served here at all.
///
/// It is the honest claim for that condition — the caller's bundle may well be a good one —
/// and it is NOT a refusal of the request: the 501 channel is what the service uses for
/// something this emulator has not been assembled to do.
pub const CA_CERTIFICATES_BUNDLE_UNREAD: &str =
    "the CA certificates bundle cannot be read: nothing in this emulator answered the read of \
     the object it names, so no trust store's certificates can be counted";

/// The modelled `AccessDenied` (HTTP 403) for a CA certificates bundle the object store
/// refuses to serve.
///
/// The sentence is the one the model documents the shape with — "Access denied." — and it
/// carries no part of the request: what the refusal states is that the read was not
/// permitted, which names neither the bucket nor the key.
pub fn ca_certificates_bundle_access_denied() -> crate::ops::types::AccessDenied {
    crate::ops::types::AccessDenied {
        message: Some("Access denied.".to_string()),
    }
}

/// One CloudFront trust store, as this emulator persists it.
///
/// The record is the single source of truth for the trust store's id, ARN, name, status,
/// certificate count, revocation-checking behaviour, the bundle source it was created from,
/// the instant it was last written and its version token — so the create and every later
/// read answer from it rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredTrustStore {
    /// `TrustStore$Id` — "The trust store's ID": the token minted at creation, which is
    /// also the key this record is held under and the last segment of its ARN.
    pub id: String,
    /// `TrustStore$Arn` — "The trust store's Amazon Resource Name (ARN)", built from the
    /// minted id once ([`trust_store_arn`]) and stored, so every read reports the same ARN.
    pub arn: String,
    /// `TrustStore$Name` — "The trust store's name", as submitted. Unique per account,
    /// which is what makes a repeated name a [`trust_store_already_exists`].
    pub name: String,
    /// [`TRUST_STORE_STATUS_PENDING`] until [`StoredTrustStore::active_due`], then
    /// [`TRUST_STORE_STATUS_ACTIVE`].
    pub status: String,
    /// The instant this trust store is owed [`TRUST_STORE_STATUS_ACTIVE`]. Stored rather
    /// than slept for; every stored trust store has one, because a trust store is stored
    /// only once its bundle has been read.
    pub active_due: SystemTime,
    /// `TrustStore$NumberOfCaCertificates` — "The trust store's number of CA certificates":
    /// the count of certificates READ out of the bundle, written once by the call that read
    /// it, so every later read reports the same count.
    pub number_of_ca_certificates: i32,
    /// `TrustStore$LastModifiedTime` — "The trust store's last modified time", written by
    /// the call that last wrote this record.
    pub last_modified_time: SystemTime,
    /// `TrustStore$UseClientCertificateOCSPEndpoint`, as submitted — or
    /// [`DEFAULT_USE_CLIENT_CERTIFICATE_OCSP_ENDPOINT`] where the create named none.
    pub use_client_certificate_ocsp_endpoint: bool,
    /// The CA certificates bundle source this trust store was created from, stored whole:
    /// the trust store IS the bundle at that address, so the record keeps the address its
    /// certificate count was read at rather than re-deriving it from a later request.
    pub bundle_source: CaCertificatesBundleSource,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this trust store.
    pub etag: String,
    /// The tags submitted with the create, held in the one representation every tagged
    /// resource of this service uses, so the tagging surface reads the very set that was
    /// stored.
    pub tags: TagSet,
}

impl StoredTrustStore {
    /// Apply the propagation transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic, like every other settlement in this store: it only ever
    /// moves the non-terminal status to the terminal one, only once the deadline has
    /// passed, so a read that calls it cannot change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == TRUST_STORE_STATUS_PENDING && now >= self.active_due {
            self.status = TRUST_STORE_STATUS_ACTIVE.to_string();
        }
    }

    /// The `TrustStore` projection every trust store operation returns, read from the one
    /// stored record.
    ///
    /// `Reason` is "The trust store's reason", which the service states for a trust store
    /// whose bundle could not be used — a condition no STORED trust store is in, because a
    /// bundle this emulator could not read is refused at the create
    /// ([`super::ca_certificates_bundle`]) instead of being stored as a failed trust store.
    /// So the member is reported absent rather than carrying an invented sentence.
    pub fn to_trust_store(&self) -> TrustStore {
        TrustStore {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            name: Some(self.name.clone()),
            status: Some(self.status.clone()),
            number_of_ca_certificates: Some(self.number_of_ca_certificates),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_time)),
            reason: None,
            use_client_certificate_ocsp_endpoint: Some(self.use_client_certificate_ocsp_endpoint),
        }
    }
}

impl ComAmazonawsCloudfrontCloudfront20200531State {
    /// Allocate a trust store id no stored trust store holds:
    /// [`TRUST_STORE_ID_PREFIX`] followed by a minted token, drawn from the same mint as
    /// every other id this service generates so no two are minted the same.
    pub fn allocate_trust_store_id(&mut self) -> String {
        loop {
            let candidate = format!("{TRUST_STORE_ID_PREFIX}{}", self.mint(ID_BODY_LEN));
            if !self.trust_stores.contains_key(&candidate) {
                break candidate;
            }
        }
    }

    /// The trust store `identifier` names, or `None` when the store holds none.
    ///
    /// A trust store operation names one by "The trust store's identifier", so the id, the
    /// ARN and the name all resolve to the one record, exactly as
    /// [`Self::distribution_tenant`] resolves a tenant's three: the id is the store's key,
    /// and the ARN and the name are compared against the STORED values rather than
    /// re-derived out of the request.
    pub fn trust_store(&self, identifier: &str) -> Option<&StoredTrustStore> {
        if let Some(trust_store) = self.trust_stores.get(identifier) {
            return Some(trust_store);
        }
        self.trust_stores
            .values()
            .find(|trust_store| trust_store.arn == identifier || trust_store.name == identifier)
    }

    /// Whether the account already holds a trust store called `name` — what makes a
    /// repeated name a [`trust_store_already_exists`] rather than a second trust store.
    ///
    /// A scan over the stored records rather than a second index beside them: those records
    /// are the single source of truth for which names are taken, so the answer cannot drift
    /// out of step with what a read reports, and the collection is bounded by
    /// [`MAX_TRUST_STORES`].
    pub fn holds_trust_store_named(&self, name: &str) -> bool {
        self.trust_stores
            .values()
            .any(|trust_store| trust_store.name == name)
    }
}
