// SPDX-License-Identifier: CC0-1.0

//! What the registry's own DATA PLANE rests on: the `docker login` credentials this
//! registry has issued, and the pull bookkeeping a pull through that plane leaves behind.
//!
//! # The credential a pull presents
//!
//! A private registry's Docker Registry v2 endpoint is reached with the credential
//! `GetAuthorizationToken` minted, not with SigV4: the token decodes to `user:password`
//! "for private registry authentication using `docker login`"
//! (`AuthorizationData$authorizationToken`), and a client presents that very pair as HTTP
//! Basic authorization. So the credential has to be one this registry can still RECOGNISE
//! when the pull arrives, which is why every issue is recorded here, and a pull presenting
//! anything else reaches no repository.
//!
//! What is recorded is NOT the password: it is the content digest of it
//! ([`crate::state::content_digest`], the digest this registry addresses content by),
//! under which the record is keyed. A pull is resolved by digesting the password it
//! presents and looking that digest up, so this registry can tell its own credential from
//! any other value without holding the secret it handed out — and the emulator's own
//! credential store, which mints the password, never goes on accepting it as SigV4
//! (`crate::ops::get_authorization_token`).
//!
//! The table is bounded ([`MAX_REGISTRY_CREDENTIALS`]) and every issue first drops what
//! the clock has already expired: a token is valid for a published 12 hours
//! ([`crate::ops::get_authorization_token::AUTHORIZATION_TOKEN_LIFETIME`]), so an expired
//! record authenticates nothing whether or not anybody looks at it.
//!
//! # What a pull records
//!
//! A pull is not a read of this registry's metadata, it is the registry's own data-plane
//! operation, and ECR states two things it records about one: `lastRecordedPullTime`, "the
//! date and time … when Amazon ECR recorded the last image pull"
//! (`ImageDetail$lastRecordedPullTime`), and the `AWS/ECR` `RepositoryPullCount` series
//! (`crate::state::pull_metrics`). Both are written by the pull itself — which is what
//! makes them observable at all — and by nothing that merely reports on an image.
//!
//! The pull time is withheld for a principal the registry's pull time update exclusion
//! list names: ECR "will not record the pull time if an excluded principal pulls an image"
//! (`RegisterPullTimeUpdateExclusion`). The count is not — the exclusion is about the pull
//! TIME of an image, not about whether the repository was pulled from.

use super::*;

/// How many issued `docker login` credentials this registry remembers at once.
///
/// See [`MAX_REPOSITORIES`] for why a generated emulator needs an entry cap at all: one
/// `GetAuthorizationToken` call adds one record, and a client may call it as often as it
/// likes. Expired records are dropped before a new one is taken
/// ([`ComAmazonawsEcrAmazonec2containerregistryV20150921State::reclaim_expired_registry_credentials`]),
/// so a client is bounded by how many tokens it takes inside ONE token lifetime rather
/// than by how many it has ever taken; past the cap the oldest issue is forgotten, which
/// costs that token its data-plane access and nothing else. The number is this emulator's
/// own bound, not a published AWS quota.
pub const MAX_REGISTRY_CREDENTIALS: usize = 1_000;

/// One `docker login` credential this registry issued: when it was issued, when it stops
/// being honoured, and the principal it was issued to.
///
/// The password itself is not here — the record is KEYED by its content digest (see the
/// module docs) — so nothing that reads this state reads the secret the caller was handed.
#[derive(Debug, Clone)]
pub struct RegistryCredential {
    /// When `GetAuthorizationToken` issued it.
    pub issued_at: SystemTime,
    /// When it stops being honoured: `issued_at` plus the published token lifetime.
    pub expires_at: SystemTime,
    /// The principal the token was issued to, as `sts:GetCallerIdentity` spells one. It is
    /// what the pull time update exclusion list is keyed by, so a pull made with this
    /// credential is excluded exactly when that principal is.
    pub principal_arn: String,
}

impl RegistryCredential {
    /// Whether the window this credential was issued for has closed as of `now`.
    pub fn has_expired(&self, now: SystemTime) -> bool {
        self.expires_at <= now
    }
}

/// The key one `docker login` password is recorded under: its content digest, the same
/// `sha256:<hex>` form this registry addresses every other piece of content it is handed
/// by. Keying by the digest is what lets a pull be resolved without this registry holding
/// the password it issued.
pub fn registry_credential_key(password: &str) -> String {
    content_digest(password.as_bytes())
}

impl ComAmazonawsEcrAmazonec2containerregistryV20150921State {
    /// Record that this registry issued the `docker login` password `password` to
    /// `principal_arn` at `issued_at`, honoured until `expires_at`.
    ///
    /// Expired records are dropped first, and the oldest issue is forgotten when the table
    /// is still at [`MAX_REGISTRY_CREDENTIALS`] afterwards, so the registry remembers the
    /// most recent issues rather than refusing to remember a new one.
    pub fn record_registry_credential(
        &mut self,
        password: &str,
        issued_at: SystemTime,
        expires_at: SystemTime,
        principal_arn: &str,
    ) {
        self.reclaim_expired_registry_credentials(issued_at);
        while self.registry_credentials.len() >= MAX_REGISTRY_CREDENTIALS {
            let Some(oldest) = self
                .registry_credentials
                .iter()
                .min_by_key(|(_key, held)| held.issued_at)
                .map(|(key, _held)| key.clone())
            else {
                break;
            };
            self.registry_credentials.remove(&oldest);
        }
        self.registry_credentials.insert(
            registry_credential_key(password),
            RegistryCredential {
                issued_at,
                expires_at,
                principal_arn: principal_arn.to_string(),
            },
        );
    }

    /// Drop every issued credential whose window has closed as of `now`.
    ///
    /// Expiry is the clock's, not a caller's: a record past its window honours nothing
    /// whether or not anything has looked at it, so dropping it changes nothing a later
    /// caller can observe.
    pub fn reclaim_expired_registry_credentials(&mut self, now: SystemTime) {
        self.registry_credentials
            .retain(|_key, held| !held.has_expired(now));
    }

    /// The record of the `docker login` password `password`, when this registry issued it
    /// and still honours it at `now`.
    ///
    /// A password this registry never issued, and one whose window has closed, both answer
    /// `None`: the data plane refuses either the same way, so a client learns nothing from
    /// the difference. A READ — the expired record it may still find is left where it is
    /// for [`Self::reclaim_expired_registry_credentials`] to drop.
    pub fn honoured_registry_credential(
        &self,
        password: &str,
        now: SystemTime,
    ) -> Option<&RegistryCredential> {
        self.registry_credentials
            .get(&registry_credential_key(password))
            .filter(|held| !held.has_expired(now))
    }

    /// Whether `principal_arn` is excluded from having its image pull times recorded: ECR
    /// "will not record the pull time if an excluded principal pulls an image"
    /// (`RegisterPullTimeUpdateExclusion`), and the exclusion list is keyed by the
    /// principal's ARN.
    pub fn pull_time_is_excluded(&self, principal_arn: &str) -> bool {
        self.pull_time_update_exclusions
            .contains_key(principal_arn)
    }

    /// Record the pull of the image `image_digest` of `repository_name`, made by
    /// `principal_arn` at `now`.
    ///
    /// The image's `lastRecordedPullTime` becomes `now`, unless that principal is excluded
    /// from pull time recording — in which case the pull happened and nothing about its
    /// time is written down, which is exactly what the exclusion means.
    pub fn record_image_pull(
        &mut self,
        repository_name: &str,
        image_digest: &str,
        principal_arn: &str,
        now: SystemTime,
    ) {
        if self.pull_time_is_excluded(principal_arn) {
            return;
        }
        if let Some(image) = self
            .repositories
            .get_mut(repository_name)
            .and_then(|repo| repo.images.get_mut(image_digest))
        {
            image.last_recorded_pull_time = Some(now);
        }
    }
}
