// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-signerdata-signerdataplane service.
//!
//! The service models exactly one operation, the `@readonly` `GetRevocationStatus`
//! read, so the revocation registry it reads from is the whole of this service's
//! state: one record per revoked signing-profile version, signing job and signing
//! certificate.
//!
//! There is no modeled write operation on the DATA plane — revocations are created by
//! the Signer CONTROL plane (`RevokeSignature` / `RevokeSigningProfile`), which is a
//! different service. A record therefore enters this store through
//! [`ComAmazonawsSignerdataSignerdataplaneState::revoke`] — the seam a host embedding
//! this emulator (a test, or the signer control-plane crate once its revoke operations
//! are implemented) uses to record a revocation whose effect the read then reports.
//! Nothing is seeded at construction: the store starts empty, so a read against an
//! emulator where nothing was ever revoked answers an EMPTY `revokedEntities` list,
//! which is what the real service answers for an artifact with no revocations.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Which registry a revocation lives in: the three kinds of entity a signature is
/// checked against. `GetRevocationStatus` takes one `profileVersionArn`, one `jobArn`
/// and a list of certificate hashes, and each is looked up in its own registry — a
/// profile-version ARN never matches a revoked job, nor a certificate hash a revoked
/// profile version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RevokedEntityKind {
    /// Keyed by signing-profile version ARN (the input's `profileVersionArn`).
    ProfileVersion,
    /// Keyed by signing-job ARN (the input's `jobArn`).
    SigningJob,
    /// Keyed by certificate hash (an entry of the input's `certificateHashes`).
    Certificate,
}

impl RevokedEntityKind {
    /// Every kind, so a caller (or a test) can walk the registries uniformly.
    pub const ALL: [Self; 3] = [Self::ProfileVersion, Self::SigningJob, Self::Certificate];

    /// The input member this kind is looked up from, for error and log text.
    pub fn input_member(self) -> &'static str {
        match self {
            Self::ProfileVersion => "profileVersionArn",
            Self::SigningJob => "jobArn",
            Self::Certificate => "certificateHashes",
        }
    }
}

/// One revocation: an entity that was revoked, from when, and why.
#[derive(Debug, Clone)]
pub struct RevocationRecord {
    /// The identifier reported in `revokedEntities` when this revocation applies.
    /// Held separately from the registry key so the reported identifier is whatever
    /// the control plane recorded, not a value derived from the request.
    pub entity_id: String,
    /// When the revocation became effective. A signature made BEFORE this instant is
    /// unaffected; one made at or after it is revoked.
    pub effective_time: SystemTime,
    /// The reason the control plane recorded, when it recorded one. Not on the wire
    /// for this operation (the model's output carries only `revokedEntities`), but
    /// part of the revocation the registry holds.
    pub reason: Option<String>,
    /// The signing platform this revocation is scoped to, when it is scoped to one.
    /// `None` applies to a signature made on any platform.
    pub platform_id: Option<String>,
}

impl RevocationRecord {
    /// A revocation of `entity_id` effective from `effective_time`, with no recorded
    /// reason and no platform scope.
    pub fn new(entity_id: impl Into<String>, effective_time: SystemTime) -> Self {
        Self {
            entity_id: entity_id.into(),
            effective_time,
            reason: None,
            platform_id: None,
        }
    }

    /// The same revocation, carrying the reason the control plane recorded.
    #[must_use]
    pub fn with_reason(mut self, reason: impl Into<String>) -> Self {
        self.reason = Some(reason.into());
        self
    }

    /// The same revocation, scoped to one signing platform: it then applies only to a
    /// signature made on that platform.
    #[must_use]
    pub fn for_platform(mut self, platform_id: impl Into<String>) -> Self {
        self.platform_id = Some(platform_id.into());
        self
    }

    /// Whether this revocation invalidates a signature made at `signature_timestamp`
    /// on `platform_id`.
    ///
    /// Two independent conditions, both of which must hold:
    ///
    /// * EFFECTIVE TIME. AWS Signer's revocation operations take an `effectiveTime`,
    ///   and a signature is treated as revoked when it was generated at or after that
    ///   instant; a signature that predates it keeps its validity. The comparison is
    ///   therefore `signature_timestamp >= effective_time` — inclusive at the instant
    ///   itself, since that instant is when the revocation takes effect.
    /// * PLATFORM SCOPE. A revocation recorded for one signing platform applies only
    ///   to signatures made on that platform; an unscoped revocation applies to all.
    pub fn applies_to(
        &self,
        signature_timestamp: &::aws_smithy_types::DateTime,
        platform_id: &str,
    ) -> bool {
        if self
            .platform_id
            .as_ref()
            .is_some_and(|scoped| scoped != platform_id)
        {
            return false;
        }
        epoch_nanos_of_timestamp(signature_timestamp) >= epoch_nanos_of_instant(self.effective_time)
    }
}

/// Per-service state for com-amazonaws-signerdata-signerdataplane. A handler reaches it through [`service_state`].
///
/// Three registries, one per [`RevokedEntityKind`], each keyed by the identifier the
/// request carries for that kind.
#[derive(Debug, Default)]
pub struct ComAmazonawsSignerdataSignerdataplaneState {
    /// Revocations keyed by signing-profile version ARN.
    pub revoked_profile_versions: HashMap<String, RevocationRecord>,
    /// Revocations keyed by signing-job ARN.
    pub revoked_jobs: HashMap<String, RevocationRecord>,
    /// Revocations keyed by certificate hash.
    pub revoked_certificates: HashMap<String, RevocationRecord>,
}

impl ComAmazonawsSignerdataSignerdataplaneState {
    /// The registry holding revocations of `kind`.
    pub fn registry(&self, kind: RevokedEntityKind) -> &HashMap<String, RevocationRecord> {
        match kind {
            RevokedEntityKind::ProfileVersion => &self.revoked_profile_versions,
            RevokedEntityKind::SigningJob => &self.revoked_jobs,
            RevokedEntityKind::Certificate => &self.revoked_certificates,
        }
    }

    /// The registry holding revocations of `kind`, mutably.
    pub fn registry_mut(
        &mut self,
        kind: RevokedEntityKind,
    ) -> &mut HashMap<String, RevocationRecord> {
        match kind {
            RevokedEntityKind::ProfileVersion => &mut self.revoked_profile_versions,
            RevokedEntityKind::SigningJob => &mut self.revoked_jobs,
            RevokedEntityKind::Certificate => &mut self.revoked_certificates,
        }
    }

    /// Record `record` as the revocation of `key` in the `kind` registry, returning the
    /// revocation that key held before, if any. This is the only way a revocation
    /// enters the store: the data plane models no write operation.
    pub fn revoke(
        &mut self,
        kind: RevokedEntityKind,
        key: impl Into<String>,
        record: RevocationRecord,
    ) -> Option<RevocationRecord> {
        self.registry_mut(kind).insert(key.into(), record)
    }

    /// Drop the revocation of `key` from the `kind` registry, returning it if it was
    /// there. The counterpart of [`revoke`](Self::revoke): what it removes stops being
    /// reported by the next read.
    pub fn unrevoke(
        &mut self,
        kind: RevokedEntityKind,
        key: &str,
    ) -> Option<RevocationRecord> {
        self.registry_mut(kind).remove(key)
    }

    /// The revocation recorded for `key` in the `kind` registry, if any.
    pub fn revocation(&self, kind: RevokedEntityKind, key: &str) -> Option<&RevocationRecord> {
        self.registry(kind).get(key)
    }

    /// The identifiers of the entities that are revoked for a signature made at
    /// `signature_timestamp` on `platform_id` by `profile_version_arn` /
    /// `job_arn`, and covered by `certificate_hashes`.
    ///
    /// Every candidate is looked up in the registry for its own kind and kept only
    /// when its record [applies](RevocationRecord::applies_to) to this signature, so
    /// the answer is computed from what is stored and is empty when nothing matches.
    /// The order is the order the request names the candidates in — the profile
    /// version, then the job, then each certificate hash — and an identifier appears
    /// at most once however many candidates map onto it.
    pub fn revoked_entities(
        &self,
        signature_timestamp: &::aws_smithy_types::DateTime,
        platform_id: &str,
        profile_version_arn: &str,
        job_arn: &str,
        certificate_hashes: &[String],
    ) -> Vec<String> {
        let candidates = [
            (RevokedEntityKind::ProfileVersion, profile_version_arn),
            (RevokedEntityKind::SigningJob, job_arn),
        ]
        .into_iter()
        .chain(
            certificate_hashes
                .iter()
                .map(|hash| (RevokedEntityKind::Certificate, hash.as_str())),
        );

        let mut revoked: Vec<String> = Vec::new();
        for (kind, key) in candidates {
            let Some(record) = self.revocation(kind, key) else {
                continue;
            };
            if !record.applies_to(signature_timestamp, platform_id) {
                continue;
            }
            if !revoked.iter().any(|seen| seen == &record.entity_id) {
                revoked.push(record.entity_id.clone());
            }
        }
        revoked
    }
}

/// Epoch nanoseconds of a stored instant, negative before the epoch.
///
/// Instants are compared in one integer space rather than through `SystemTime`
/// arithmetic so a revocation effective before 1970 (or a signature timestamp that is)
/// compares correctly instead of saturating at the epoch.
fn epoch_nanos_of_instant(at: SystemTime) -> i128 {
    match at.duration_since(UNIX_EPOCH) {
        Ok(since) => nanos_as_i128(since),
        Err(before) => -nanos_as_i128(before.duration()),
    }
}

/// Epoch nanoseconds of a modelled `Timestamp`.
///
/// `aws_smithy_types::DateTime` holds whole epoch seconds (signed) plus a
/// non-negative sub-second nanosecond part, so the two combine additively for
/// instants on both sides of the epoch.
fn epoch_nanos_of_timestamp(when: &::aws_smithy_types::DateTime) -> i128 {
    i128::from(when.secs()) * 1_000_000_000 + i128::from(when.subsec_nanos())
}

/// `duration` in nanoseconds, saturating at `i128::MAX` rather than wrapping.
fn nanos_as_i128(duration: Duration) -> i128 {
    i128::try_from(duration.as_nanos()).unwrap_or(i128::MAX)
}

/// The message for the one failure that is not about the request: this service was
/// mounted on an engine that holds no store for it.
pub const NO_STORE: &str =
    "the com-amazonaws-signerdata-signerdataplane revocation registry is not mounted on this engine";

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSignerdataSignerdataplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSignerdataSignerdataplaneState>)
}
