// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-signer-wallabyservice service.
//!
//! AWS Signer has two resources, and both live here:
//!
//! * a **signing profile** — the code-signing template `PutSigningProfile` mints a
//!   version of, `GetSigningProfile` / `ListSigningProfiles` report,
//!   `CancelSigningProfile` moves to `Canceled` and `RevokeSigningProfile` moves
//!   to `Revoked` (Signer never hard-deletes a profile, so there is no removal
//!   path);
//! * a **signing job** — started by `StartSigningJob` against a profile whose
//!   platform, version and signing material it SNAPSHOTS, reported by
//!   `DescribeSigningJob`, and carrying the revocation `RevokeSignature` records
//!   on it once its signature is no longer trusted.
//!
//! Every handler reads and writes the one record below through [`service_state`];
//! nothing in this service answers from a literal.
//!
//! Both resources are REGIONAL, as AWS Signer is: a profile and the jobs, tags
//! and permissions that hang off it belong to the region whose endpoint created
//! them, and no other region's endpoint resolves them (see [`ProfileKey`]).

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Destination, EncryptionAlgorithmOptions, HashAlgorithmOptions, S3SignedObject,
    SignatureValidityPeriod, SignedObject, SigningConfiguration, SigningImageFormat, SigningJob,
    SigningJobRevocationRecord, SigningMaterial, SigningPlatform, SigningPlatformOverrides,
    SigningProfile, SigningProfileRevocationRecord, Source,
};

/// The region an ARN is built for when the request carries no SigV4 credential
/// scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `SigningProfileStatus`'s values, spelled as the model spells them (see
/// [`crate::ops::types::SIGNING_PROFILE_STATUS_VALUES`]).
pub const PROFILE_STATUS_ACTIVE: &str = "Active";
/// The status `CancelSigningProfile` moves a profile to.
pub const PROFILE_STATUS_CANCELED: &str = "Canceled";
/// The status `RevokeSigningProfile` moves a profile to.
pub const PROFILE_STATUS_REVOKED: &str = "Revoked";

// Every Signer error shape models BOTH `message` (prose) and `code` (`ErrorCode`,
// `smithy_query(query="shape",shape="ResourceNotFoundException")`), and a client
// branches on the code rather than on the prose — so every error this service
// answers with names the condition it refused in `code`. The codes below are the
// ones Signer publishes for the conditions this crate can refuse; a refusal of
// the request's own values carries the error SHAPE's name instead, which is what
// that shape already says.

/// No profile of that name (or not one this account holds).
pub const ERROR_CODE_PROFILE_NOT_FOUND: &str = "ProfileNotFound";
/// A SUB-RESOURCE of a profile the request's own values named — a version of the
/// profile, one of its permission statements — does not exist, while the profile
/// itself was resolved. Signer publishes no per-resource code for these, so the
/// refusal carries the modelled shape's own name; the profile being absent is a
/// DIFFERENT condition and carries [`ERROR_CODE_PROFILE_NOT_FOUND`], so a client
/// branching on `code` can tell "no such profile" from "no such part of it".
pub const ERROR_CODE_NOT_FOUND: &str = "ResourceNotFoundException";
/// No signing job of that id.
pub const ERROR_CODE_JOB_NOT_FOUND: &str = "JobNotFound";
/// No signing platform of that id — this service publishes a fixed catalog, so a
/// `platformId` outside it names no platform at all. Spelled the way the other
/// not-found codes in this service are (`ProfileNotFound`, `JobNotFound`): the
/// resource that was not found, followed by `NotFound`.
pub const ERROR_CODE_PLATFORM_NOT_FOUND: &str = "PlatformNotFound";
/// A `nextToken` that is not one this service handed out.
pub const ERROR_CODE_INVALID_NEXT_TOKEN: &str = "InvalidNextToken";
/// A request refused for the values it carried, rather than for a condition of
/// the resource: the modelled shape's own name.
pub const ERROR_CODE_VALIDATION: &str = "ValidationException";
/// A tagging request refused for the values it carried — the modelled shape's own
/// name, which is the shape the tagging operations declare for a bad request
/// (`BadRequestException`, "The request contains invalid parameters for the ARN or
/// tags").
pub const ERROR_CODE_BAD_REQUEST: &str = "BadRequestException";
/// The store could not be reached — this emulator's own fault, not the request's.
pub const ERROR_CODE_INTERNAL_SERVICE_ERROR: &str = "InternalServiceErrorException";

/// The error code a refusal carries when the PROFILE'S STATUS is what forbids
/// the call: `Profile` followed by that status, which is how Signer spells the
/// refusal of a canceled or revoked profile (`ProfileRevoked` for a profile in
/// `Revoked`). `SigningProfileStatus` is `Active | Canceled | Revoked`, so the
/// code is derived from the stored status rather than written out per call site.
pub fn profile_status_error_code(status: &str) -> String {
    format!("Profile{status}")
}

/// `SigningStatus`'s values, spelled as the model spells them (see
/// [`crate::ops::types::SIGNING_STATUS_VALUES`]).
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";
/// The status a signing job reports once the clock has made its completion due.
pub const JOB_STATUS_SUCCEEDED: &str = "Succeeded";

/// The largest page `ListSigningProfiles` serves, and the page size it uses when
/// the request names no `maxResults`: `ListSigningProfilesRequest$maxResults` is
/// modelled `@range(min: 1, max: 25)`.
pub const PROFILE_PAGE_LIMIT: i32 = 25;

/// The largest page `ListSigningPlatforms` serves, and the page size it uses
/// when the request names no `maxResults`:
/// `ListSigningPlatformsRequest$maxResults` is modelled
/// `@range(min: 1, max: 25)`.
///
/// Stated per operation rather than shared with [`PROFILE_PAGE_LIMIT`], which
/// carries the same bound today: each is the range ITS own request member is
/// modelled with, so one operation's model changing does not silently move the
/// other's page size.
pub const PLATFORM_PAGE_LIMIT: i32 = 25;

/// The largest page `ListSigningJobs` serves, and the page size it uses when the
/// request names no `maxResults`: `ListSigningJobsRequest$maxResults` is modelled
/// `@range(min: 1, max: 25)`.
///
/// Stated per operation rather than shared with the other two page limits, for
/// the reason [`PLATFORM_PAGE_LIMIT`] gives.
pub const JOB_PAGE_LIMIT: i32 = 25;

/// How many resources one page of a paginated read carries: the `maxResults` the
/// request named, or `limit` — the operation's own page limit — when it named
/// none.
///
/// Every paginated read of this service models `maxResults` with a range whose
/// `max` is that operation's page limit ([`PROFILE_PAGE_LIMIT`],
/// [`PLATFORM_PAGE_LIMIT`], [`JOB_PAGE_LIMIT`], each kept per operation so one
/// operation's model changing does not move another's page size), and the
/// boundary validator refused anything outside that range before a handler ran,
/// so the only case left here is an absent value: serve the largest page the
/// model allows. The `clamp` states that invariant rather than correcting a
/// caller — a value reaching here outside the range is a validator bug, which
/// the debug assertion surfaces instead of silently repairing.
pub fn page_size(requested: Option<i32>, limit: i32) -> usize {
    let requested = requested.unwrap_or(limit);
    debug_assert!(
        (1..=limit).contains(&requested),
        "maxResults {requested} should have been refused at the boundary"
    );
    usize::try_from(requested.clamp(1, limit))
        .expect("a page size clamped into the modelled range fits in usize")
}

/// The most tags one signing profile can hold.
///
/// Not a chosen number: `TagMap` — the shape `TagResource` writes and both
/// `ListTagsForResource` and `GetSigningProfile` report — is modelled
/// `@length(min: 1, max: 200)`
/// (`smithy_query(query="shape", shape="TagMap")`), so a profile carrying more
/// than 200 tags is one no read of it could report.
pub const MAX_TAGS_PER_PROFILE: usize = 200;

/// The prefix a tag key may not carry: AWS reserves `aws:` for its own tags.
///
/// `TagKey` is modelled `@pattern("^(?!aws:)[a-zA-Z+-=._:/]+$")`
/// (`smithy_query(query="shape", shape="TagMap")`) — a negative lookahead, which
/// the boundary validator's regex engine cannot express, so the rule holds here.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// The fewest characters a tag key can carry: `TagKey` is modelled
/// `@length(min: 1, max: 128)` (`smithy_query(query="shape", shape="TagKey")`).
///
/// The bound is enforced in this crate because a map KEY is not a constrained
/// leaf at the framework boundary (only a map's values and a list's members are),
/// so nothing ahead of a handler judges the key of `TagMap`.
pub const TAG_KEY_MIN_LENGTH: usize = 1;

/// The most characters a tag key can carry — the `max` of the same modelled
/// `@length` as [`TAG_KEY_MIN_LENGTH`].
pub const TAG_KEY_MAX_LENGTH: usize = 128;

/// The `ValidityType` unit the default signature validity period is counted in,
/// spelled as the model spells it (see
/// [`crate::ops::types::VALIDITY_TYPE_VALUES`]).
pub const VALIDITY_TYPE_MONTHS: &str = "MONTHS";

/// How many [`VALIDITY_TYPE_MONTHS`] a profile's signatures stay valid for when
/// the `PutSigningProfile` that created it named no period.
///
/// Not a chosen number: the model documents
/// `PutSigningProfileRequest$signatureValidityPeriod` as "The default validity
/// period override for any signature generated using this signing profile. If
/// unspecified, the default is 135 months."
/// (`smithy_query(query="doc", shape="PutSigningProfileRequest",
/// member="signatureValidityPeriod")`), so a profile created without one still
/// HAS a validity period and every read of it reports that period.
pub const DEFAULT_SIGNATURE_VALIDITY_MONTHS: i32 = 135;

/// The validity period a profile gets when its `PutSigningProfile` named none —
/// [`DEFAULT_SIGNATURE_VALIDITY_MONTHS`] months, the documented default.
pub fn default_signature_validity_period() -> SignatureValidityPeriod {
    SignatureValidityPeriod {
        value: DEFAULT_SIGNATURE_VALIDITY_MONTHS,
        r#type: Some(VALIDITY_TYPE_MONTHS.to_string()),
    }
}

/// How long a signing job stays `InProgress` — the status `StartSigningJob`
/// returns it in — before the clock owes it `Succeeded`.
///
/// Signing takes AWS as long as the payload needs; the emulator chooses a span,
/// and it is deliberately NOT zero so a caller that describes the job straight
/// after starting it still observes `InProgress`. The due instant is stored on
/// the record and materialised by whichever later call reads it — nothing sleeps.
pub const JOB_COMPLETION_SPAN: Duration = Duration::from_millis(500);

/// The key one signing profile — and everything that belongs to it — is held
/// under: the region the resource lives in, then its `profileName`.
///
/// AWS Signer is a regional service, so a profile is a resource OF the region
/// whose endpoint created it: the same name in two regions names two independent
/// profiles, and neither is reachable through the other's endpoint. The region
/// comes first so the keys of one region are a contiguous range of the map,
/// ordered by name — the stable total order the profile pagination token indexes
/// into.
pub type ProfileKey = (String, String);

/// The [`ProfileKey`] of `profile_name` in `region`.
pub fn profile_key(region: &str, profile_name: &str) -> ProfileKey {
    (region.to_string(), profile_name.to_string())
}

/// One signing profile, as this emulator persists it.
///
/// `arn` and `profile_version_arn` are stored rather than recomputed per read;
/// the region they name is the one the profile is keyed under (see
/// [`ProfileKey`]), so a profile's own ARN always names the region whose endpoint
/// serves it.
#[derive(Debug, Clone)]
pub struct SigningProfileRecord {
    pub profile_name: String,
    /// The current version, matching `ProfileVersion`'s `^[a-zA-Z0-9]{10}$`.
    pub profile_version: String,
    pub arn: String,
    /// `<arn>/<profile_version>`.
    pub profile_version_arn: String,
    /// The opaque, server-generated id of this profile, minted when the profile
    /// was first created and kept across every later version of it.
    ///
    /// `GetSigningProfile` reports it as `profileToken` and `PutSigningProfile`
    /// as its answer's `tags.internalId`: ONE id per profile, so the two reports
    /// can never drift apart.
    pub profile_token: String,
    pub platform_id: String,
    pub platform_display_name: Option<String>,
    pub signing_material: Option<SigningMaterial>,
    pub signature_validity_period: Option<SignatureValidityPeriod>,
    pub overrides: Option<SigningPlatformOverrides>,
    pub signing_parameters: BTreeMap<String, String>,
    /// One of `SigningProfileStatus`'s values.
    pub status: String,
    pub status_reason: Option<String>,
    /// What `RevokeSigningProfile` recorded, for a profile that has been revoked;
    /// `None` for one that has not.
    pub revocation_record: Option<SigningProfileRevocationRecord>,
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// Every version this profile has had, oldest first; the last is
    /// [`Self::profile_version`].
    pub versions: Vec<String>,
}

impl SigningProfileRecord {
    /// Whether this profile can start new signing jobs.
    ///
    /// `CancelSigningProfile` documents that a canceled profile "is still
    /// viewable with the ListSigningProfiles operation, but it cannot perform new
    /// signing jobs", and a revoked one signs nothing either.
    pub fn can_sign(&self) -> bool {
        self.status == PROFILE_STATUS_ACTIVE
    }

    /// Whether this profile has been revoked.
    pub fn is_revoked(&self) -> bool {
        self.status == PROFILE_STATUS_REVOKED
    }

    /// Whether a tagging API call on this profile is admitted at all.
    ///
    /// `BadRequestException` documents the one status that forbids one: the shape
    /// "also occurs when you call a tagging API on a cancelled signing profile"
    /// (`smithy_query(query="shape", shape="BadRequestException")`) — the whole
    /// tagging route, the calls that REPORT the map as much as the ones that
    /// change it. Nothing documents a refusal for any other status, and a
    /// canceled profile is the one Signer schedules for deletion, so `Active` and
    /// `Revoked` profiles admit every tagging call.
    ///
    /// Private: [`Self::tagging_refusal`] is the one entry point the route's
    /// decision is taken through, so no caller can ask the question and forget
    /// the answer.
    fn admits_tagging_calls(&self) -> bool {
        self.status != PROFILE_STATUS_CANCELED
    }

    /// The `(message, code)` a tagging call on this profile is refused with, or
    /// `None` when the status the store holds NOW admits the call.
    ///
    /// ONE decision for the whole tagging route, so every operation bound to it
    /// answers the same shape, message and `code` for the same profile.
    pub fn tagging_refusal(&self) -> Option<(String, String)> {
        if self.admits_tagging_calls() {
            return None;
        }
        Some((
            tagging_refused_message(&self.profile_name, &self.status),
            profile_status_error_code(&self.status),
        ))
    }

    /// The stored tags, as the wire map — absent when there are none.
    ///
    /// `TagMap` is modelled `@length(min: 1, max: 200)`, so a member that only
    /// LISTS profiles reports no map at all for an untagged one rather than an
    /// empty one, which would violate that bound. An operation that reports ONE
    /// profile reports the map itself — see [`Self::reported_tag_map`].
    pub fn tag_map(&self) -> Option<HashMap<String, String>> {
        non_empty_map(&self.tags)
    }

    /// This profile's tags, as the wire map `GetSigningProfile` reports — ALWAYS
    /// present, and empty for a profile carrying no tags.
    ///
    /// A reader of one profile has to be able to tell "this profile has no tags"
    /// from "this answer does not report tags", which an absent member cannot
    /// say; the recorded `GetSigningProfile` answer for an untagged profile
    /// carries `tags` as a present, empty object.
    pub fn reported_tag_map(&self) -> HashMap<String, String> {
        self.tags
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()
    }

    /// Record a revocation of this profile: it moves to `Revoked`, reports the
    /// caller's `reason` as its `statusReason`, and carries the revocation record
    /// every later read reports.
    ///
    /// `revocation_effective_from` is the caller's `effectiveTime` — the instant
    /// signatures stop being trusted — while `revoked_at` is when this call
    /// happened, which is why the two are stored separately.
    ///
    /// A revocation is a fact about trust that has already ended, so it is
    /// recorded ONCE: revoking an already-revoked profile leaves the first record
    /// standing rather than restating when, from which instant, why and by whom
    /// trust ended. Moving `revocation_effective_from` forward would otherwise
    /// restore trust in signatures the first revocation had already placed after
    /// it.
    pub fn revoke(
        &mut self,
        reason: &str,
        effective_time: ::aws_smithy_types::DateTime,
        revoked_by: Option<String>,
        now: SystemTime,
    ) {
        if self.is_revoked() {
            return;
        }
        self.status = PROFILE_STATUS_REVOKED.to_string();
        self.status_reason = Some(reason.to_string());
        self.revocation_record = Some(SigningProfileRevocationRecord {
            revocation_effective_from: Some(effective_time),
            revoked_at: Some(to_timestamp(now)),
            revoked_by,
        });
    }

    /// Carry the revocation `previous` holds onto this record, which is about to
    /// REPLACE it under the same name.
    ///
    /// A revocation is a fact about trust that has already ended (see
    /// [`Self::revoke`]), so nothing a later caller writes to the profile restores
    /// that trust: a profile that was revoked stays `Revoked` and keeps the one
    /// record of from which instant, why and by whom trust ended, however much of
    /// the rest of it the replacing write rebuilds. Every version the profile has
    /// had therefore stays covered by it — which is what
    /// [`Self::version_revoked_at`] reports — rather than becoming trusted again
    /// because the profile was written to. A `previous` nobody has revoked carries
    /// nothing to keep.
    pub fn carry_revocation_forward(&mut self, previous: &Self) {
        if !previous.is_revoked() {
            return;
        }
        self.status = previous.status.clone();
        self.status_reason = previous.status_reason.clone();
        self.revocation_record = previous.revocation_record.clone();
    }

    /// This profile's ARN for `version` — the `profileVersionArn` a read of that
    /// version reports, which is the profile's own ARN with the version after it
    /// (see [`Self::profile_version_arn`], the same form for the current one).
    pub fn version_arn(&self, version: &str) -> String {
        format!("{}/{version}", self.arn)
    }

    /// Whether this profile's revocation leaves a signature that `version`
    /// produced at `signature_timestamp` untrusted.
    ///
    /// `RevokeSigningProfile` revokes the PROFILE — it moves the profile itself
    /// to `Revoked` and records ONE revocation record on it — so the revocation
    /// covers every version the profile has had, from the
    /// `revocationEffectiveFrom` instant the caller named: "the time when
    /// revocation becomes effective", so a signature made at or after it is no
    /// longer trusted while one made before it keeps its validity. A version this
    /// profile has never had names nothing that record covers.
    pub fn version_revoked_at(
        &self,
        version: &str,
        signature_timestamp: &::aws_smithy_types::DateTime,
    ) -> bool {
        if !self.versions.iter().any(|held| held == version) {
            return false;
        }
        let Some(record) = self.revocation_record.as_ref() else {
            return false;
        };
        // A record naming no effective instant names no instant trust ended AT,
        // so what it revokes is revoked whenever it was signed.
        record
            .revocation_effective_from
            .as_ref()
            .is_none_or(|effective_from| at_or_after(signature_timestamp, effective_from))
    }

    /// The stored signing parameters, as the wire map — absent when there are none.
    pub fn signing_parameter_map(&self) -> Option<HashMap<String, String>> {
        non_empty_map(&self.signing_parameters)
    }

    /// The `SigningProfile` summary `ListSigningProfiles` returns.
    pub fn to_summary(&self) -> SigningProfile {
        SigningProfile {
            profile_name: Some(self.profile_name.clone()),
            profile_version: Some(self.profile_version.clone()),
            profile_version_arn: Some(self.profile_version_arn.clone()),
            signing_material: self.signing_material.clone(),
            signature_validity_period: self.signature_validity_period.clone(),
            platform_id: Some(self.platform_id.clone()),
            platform_display_name: self.platform_display_name.clone(),
            signing_parameters: self.signing_parameter_map(),
            status: Some(self.status.clone()),
            arn: Some(self.arn.clone()),
            tags: self.tag_map(),
        }
    }
}

/// One signing job, as this emulator persists it.
///
/// The platform, profile version and signing material are COPIES taken when the
/// job started: `DescribeSigningJob` reports what the job ran with, which a later
/// `PutSigningProfile` on the same profile must not change.
#[derive(Debug, Clone)]
pub struct SigningJobRecord {
    pub job_id: String,
    /// The region this job was started in — the one whose endpoint reports it.
    /// A job belongs to the region of the profile it signs with, which is the
    /// region the `StartSigningJob` was signed for.
    pub region: String,
    pub profile_name: String,
    pub profile_version: String,
    pub platform_id: String,
    pub platform_display_name: Option<String>,
    pub source: Source,
    pub destination: Destination,
    pub signed_object: Option<SignedObject>,
    pub signing_material: Option<SigningMaterial>,
    pub signing_parameters: BTreeMap<String, String>,
    pub overrides: Option<SigningPlatformOverrides>,
    /// One of `SigningStatus`'s values.
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub completed_at: Option<SystemTime>,
    /// The instant the clock owes this job its terminal status, while it is still
    /// `InProgress`.
    pub completion_due: Option<SystemTime>,
    pub requested_by: Option<String>,
    pub job_owner: String,
    /// The AWS account that invoked this job — the account whose credentials made
    /// the call that started it, which `jobInvoker` reports and the job listing's
    /// invoker filter is compared against. `None` only for a job no credential
    /// was resolved for.
    pub job_invoker: Option<String>,
    pub client_request_token: String,
    /// The revocation of the signature this job produced, once somebody has
    /// revoked it; `None` while the signature is still trusted. `SigningStatus`
    /// is `InProgress | Failed | Succeeded` and cannot say "revoked", so this
    /// record — reported as `revocationRecord`, and summarised as `isRevoked`
    /// ("Indicates whether the signing job is revoked") — is what carries the
    /// fact.
    pub revocation_record: Option<SigningJobRevocationRecord>,
}

impl SigningJobRecord {
    /// Materialise the completion the clock owes this job, if `now` is past its
    /// due instant.
    ///
    /// `completed_at` is the instant the job FELL DUE, not the instant somebody
    /// looked: the work finished whether or not anyone was reading, so two
    /// readers of the same job see the same completion time.
    pub fn settle(&mut self, now: SystemTime) {
        let Some(due) = self.completion_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = JOB_STATUS_SUCCEEDED.to_string();
        self.completed_at = Some(due);
        self.signed_object = Some(signed_object_of(&self.destination, &self.job_id));
        self.completion_due = None;
    }

    /// Record the revocation of the signature this job produced: the job carries
    /// the caller's `reason`, the instant of the revoking call and the identity
    /// that revoked it, which every later read of the job reports.
    ///
    /// A revocation is a fact about a signature that has already stopped being
    /// trusted, so it is recorded ONCE: revoking an already-revoked job leaves
    /// the first record standing rather than restating when, why and by whom
    /// trust ended.
    pub fn revoke(&mut self, reason: &str, revoked_by: Option<String>, now: SystemTime) {
        if self.is_revoked() {
            return;
        }
        self.revocation_record = Some(SigningJobRevocationRecord {
            reason: Some(reason.to_string()),
            revoked_at: Some(to_timestamp(now)),
            revoked_by,
        });
    }

    /// Whether the signature this job produced has been revoked.
    pub fn is_revoked(&self) -> bool {
        self.revocation_record.is_some()
    }

    /// This job's ARN — the form a request that names a signing job by ARN
    /// carries it in (see [`job_arn`]).
    pub fn arn(&self) -> String {
        job_arn(&self.job_id, &self.region)
    }

    /// The stored signing parameters, as the wire map — absent when there are none.
    pub fn signing_parameter_map(&self) -> Option<HashMap<String, String>> {
        non_empty_map(&self.signing_parameters)
    }

    /// The instant the signature this job produced expires — the
    /// `signatureExpiresAt` a read of the job reports.
    ///
    /// The ONE place that instant is taken from, so the member a read of a single
    /// job reports and the window a listing's expiry bounds judge can never
    /// disagree. Nothing in this service records it on a job yet: a signature
    /// expires with the certificate it was issued under, and no certificate is
    /// issued here, so a read reports the member absent rather than an instant
    /// nothing fixed — and a listing bounded by an expiry judges that same
    /// absence instead of guessing one.
    pub fn signature_expires_at(&self) -> Option<::aws_smithy_types::DateTime> {
        None
    }

    /// The `SigningJob` summary `ListSigningJobs` returns.
    ///
    /// Every member is read off this record, so a listed job carries the same
    /// values `DescribeSigningJob` reports for it, narrowed to the members the
    /// summary shape holds.
    pub fn to_summary(&self) -> SigningJob {
        SigningJob {
            job_id: Some(self.job_id.clone()),
            source: Some(self.source.clone()),
            signed_object: self.signed_object.clone(),
            signing_material: self.signing_material.clone(),
            created_at: Some(to_timestamp(self.created_at)),
            status: Some(self.status.clone()),
            // `SigningStatus` cannot say "revoked", so the summary reports the
            // fact of the revocation record this job carries.
            is_revoked: self.is_revoked(),
            profile_name: Some(self.profile_name.clone()),
            profile_version: Some(self.profile_version.clone()),
            platform_id: Some(self.platform_id.clone()),
            platform_display_name: self.platform_display_name.clone(),
            signature_expires_at: self.signature_expires_at(),
            job_owner: Some(self.job_owner.clone()),
            job_invoker: self.job_invoker.clone(),
        }
    }
}

/// The cross-account permission policy of a signing profile: the statements
/// `AddProfilePermission` writes, the revision chain they move along and the
/// document whose size `ListProfilePermissions` reports.
pub mod permissions;

/// Per-service state for com-amazonaws-signer-wallabyservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSignerWallabyserviceState {
    /// Every signing profile, keyed by [`ProfileKey`] — the region it lives in
    /// and the `profileName` every operation and every profile ARN resolves
    /// through. A `BTreeMap` is what gives `ListSigningProfiles` the stable total
    /// order its pagination token indexes into, within one region.
    pub profiles: BTreeMap<ProfileKey, SigningProfileRecord>,
    /// Every signing job, keyed by `jobId`; ids sort in the order they were
    /// minted. Which region a job belongs to is [`SigningJobRecord::region`] —
    /// a job id is unique across the store, so another region's id resolves to no
    /// job at all rather than to a different one.
    pub jobs: BTreeMap<String, SigningJobRecord>,
    /// A `StartSigningJob` `clientRequestToken` to the job that token started, so
    /// a retried start answers with the first job instead of a second one. Keyed
    /// by `(region, clientRequestToken)`: the token is a fact of the region's
    /// endpoint that served it, so replaying it elsewhere starts that region's own
    /// job.
    pub job_tokens: HashMap<(String, String), String>,
    /// The cross-account permission policy of every profile that has one, keyed by
    /// the same [`ProfileKey`] [`Self::profiles`] uses, so a policy always
    /// resolves through the profile it belongs to.
    pub profile_permissions: BTreeMap<ProfileKey, permissions::ProfilePermissionPolicy>,
    /// The counter profile versions are minted from.
    pub profile_version_seq: u64,
    /// The counter profile tokens are minted from.
    pub profile_token_seq: u64,
    /// The counter job ids are minted from.
    pub job_id_seq: u64,
    /// The counter permission-policy revision ids are minted from.
    pub permission_revision_seq: u64,
}

impl ComAmazonawsSignerWallabyserviceState {
    /// Mint an unused profile version matching `ProfileVersion`'s
    /// `^[a-zA-Z0-9]{10}$`.
    ///
    /// Ten base-62 digits of a strictly increasing counter, so no two versions of
    /// any profile collide and a second `PutSigningProfile` on one name always
    /// yields a new version.
    pub fn mint_profile_version(&mut self) -> String {
        self.profile_version_seq = self.profile_version_seq.wrapping_add(1);
        base62_10(self.profile_version_seq)
    }

    /// Mint an unused job id.
    ///
    /// `JobId` is an unconstrained string that AWS renders as a UUID, so the
    /// counter is rendered in that shape — a client that parses a job id as a
    /// UUID still works.
    pub fn mint_job_id(&mut self) -> String {
        loop {
            self.job_id_seq = self.job_id_seq.wrapping_add(1);
            let id = uuid_shaped(JOB_ID_NAMESPACE, self.job_id_seq);
            if !self.jobs.contains_key(&id) {
                return id;
            }
        }
    }

    /// Mint an opaque profile token — the server-generated id a profile keeps for
    /// its whole life.
    ///
    /// Opaque means a client may only carry it around: nothing in this service
    /// resolves a profile BY token, so the only requirements are that it is
    /// unique and that it looks like what AWS mints, a UUID. Its namespace
    /// differs from [`Self::mint_job_id`]'s, so a token can never be mistaken for
    /// a job id.
    pub fn mint_profile_token(&mut self) -> String {
        self.profile_token_seq = self.profile_token_seq.wrapping_add(1);
        uuid_shaped(PROFILE_TOKEN_NAMESPACE, self.profile_token_seq)
    }

    /// Mint an unused permission-policy revision id.
    ///
    /// `revisionId` is an unconstrained string that AWS mints as a UUID, so the
    /// counter is rendered in that shape; its namespace differs from the job-id
    /// and profile-token ones, so a revision id can never be mistaken for either.
    /// Strictly increasing, so a revision this mints is never one a client is
    /// already holding.
    pub fn mint_permission_revision_id(&mut self) -> String {
        self.permission_revision_seq = self.permission_revision_seq.wrapping_add(1);
        uuid_shaped(PERMISSION_REVISION_NAMESPACE, self.permission_revision_seq)
    }

    /// The profile `profile_name` names IN `region`, or `None` when that region
    /// holds none of that name.
    pub fn profile(&self, region: &str, profile_name: &str) -> Option<&SigningProfileRecord> {
        self.profiles.get(&profile_key(region, profile_name))
    }

    /// [`Self::profile`], for a call that CHANGES the profile.
    pub fn profile_mut(
        &mut self,
        region: &str,
        profile_name: &str,
    ) -> Option<&mut SigningProfileRecord> {
        self.profiles.get_mut(&profile_key(region, profile_name))
    }

    /// Store `record` as `region`'s profile of that name, replacing whatever that
    /// region held under it; another region's profile of the same name is left
    /// alone.
    ///
    /// Replacing is not un-revoking: the revocation the replaced profile holds is
    /// carried onto `record` ([`SigningProfileRecord::carry_revocation_forward`]),
    /// so no write to a name reported as revoked makes it trusted again.
    pub fn insert_profile(&mut self, region: &str, mut record: SigningProfileRecord) {
        let key = profile_key(region, &record.profile_name);
        if let Some(previous) = self.profiles.get(&key) {
            record.carry_revocation_forward(previous);
        }
        self.profiles.insert(key, record);
    }

    /// Every profile `region` holds, as `(profileName, profile)` in name order —
    /// and nothing from any other region.
    pub fn profiles_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = (&'a str, &'a SigningProfileRecord)> + 'a {
        self.profiles
            .range(profile_key(region, "")..)
            .take_while(move |((profile_region, _), _)| profile_region == region)
            .map(|((_, profile_name), profile)| (profile_name.as_str(), profile))
    }

    /// The permission policy of `region`'s profile of that name, or `None` when
    /// nobody has granted anything on it.
    pub fn profile_policy(
        &self,
        region: &str,
        profile_name: &str,
    ) -> Option<&permissions::ProfilePermissionPolicy> {
        self.profile_permissions
            .get(&profile_key(region, profile_name))
    }

    /// [`Self::profile_policy`], for a call that CHANGES an EXISTING policy.
    pub fn profile_policy_mut(
        &mut self,
        region: &str,
        profile_name: &str,
    ) -> Option<&mut permissions::ProfilePermissionPolicy> {
        self.profile_permissions
            .get_mut(&profile_key(region, profile_name))
    }

    /// Drop the permission policy of `region`'s profile of that name.
    ///
    /// A resource policy exists only while it holds a statement, so the write
    /// that takes the LAST statement out leaves no policy behind rather than an
    /// empty one — which is what makes every later read of it report the policy
    /// as absent instead of as present and empty.
    pub fn drop_profile_policy(&mut self, region: &str, profile_name: &str) {
        self.profile_permissions
            .remove(&profile_key(region, profile_name));
    }

    /// [`Self::profile_policy`], for a call that CHANGES the policy, creating the
    /// empty policy when the profile holds none yet.
    pub fn profile_policy_entry(
        &mut self,
        region: &str,
        profile_name: &str,
    ) -> &mut permissions::ProfilePermissionPolicy {
        self.profile_permissions
            .entry(profile_key(region, profile_name))
            .or_default()
    }

    /// The job `job_id` names in `region`, exactly as it is stored.
    ///
    /// For a REPORTING read whose answer does not depend on the job's status: it
    /// changes nothing, so what a later caller sees is untouched by the asking. A
    /// read that reports the status takes [`Self::settled_job_mut`] instead,
    /// which first materialises the completion the clock already owes the job.
    pub fn job(&self, region: &str, job_id: &str) -> Option<&SigningJobRecord> {
        self.jobs.get(job_id).filter(|job| job.region == region)
    }

    /// The job `job_id` names in `region`, with any completion the clock owes it
    /// already materialised.
    pub fn settled_job_mut(
        &mut self,
        region: &str,
        job_id: &str,
        now: SystemTime,
    ) -> Option<&mut SigningJobRecord> {
        let job = self
            .jobs
            .get_mut(job_id)
            .filter(|job| job.region == region)?;
        job.settle(now);
        Some(job)
    }

    /// Materialise every completion the clock has already made due for a job of
    /// `region`.
    ///
    /// For a read that REPORTS job statuses: each completion was owed at its due
    /// instant whether or not anybody asked, so a listing reports the status each
    /// job holds at `now` rather than the one it held when it was last written —
    /// and two readers of one job still see the same completion time (see
    /// [`SigningJobRecord::settle`]).
    pub fn settle_jobs_in(&mut self, region: &str, now: SystemTime) {
        for job in self.jobs.values_mut() {
            if job.region == region {
                job.settle(now);
            }
        }
    }

    /// Every job `region` holds, in the order their ids were minted — which is
    /// the order they were started, and the one stable total order a page of a
    /// job listing is cut out of — and nothing from any other region.
    pub fn jobs_in<'a>(
        &'a self,
        region: &'a str,
    ) -> impl Iterator<Item = &'a SigningJobRecord> + 'a {
        self.jobs.values().filter(move |job| job.region == region)
    }

    /// The job `region`'s endpoint already started for `client_request_token`, so
    /// a repeated token answers with that job instead of starting a second one.
    pub fn job_for_token(
        &self,
        region: &str,
        client_request_token: &str,
    ) -> Option<&SigningJobRecord> {
        let job_id = self
            .job_tokens
            .get(&(region.to_string(), client_request_token.to_string()))?;
        self.jobs.get(job_id)
    }

    /// Remember that `region`'s `client_request_token` started `job_id`.
    pub fn record_job_token(&mut self, region: &str, client_request_token: &str, job_id: String) {
        self.job_tokens
            .insert((region.to_string(), client_request_token.to_string()), job_id);
    }

    /// The profile a tagging call's `resourceArn` names, as served by `region`'s
    /// endpoint.
    ///
    /// Resolution is by the name the ARN carries, confirmed against the ARN the
    /// profile was STORED with — so a label naming another account's or another
    /// region's signing profile resolves to nothing rather than to this region's
    /// profile of the same name. The ARN's own region has to be the region whose
    /// endpoint is answering, because that is the only region whose resources
    /// that endpoint holds.
    pub fn profile_for_tagging(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Result<&SigningProfileRecord, TaggingArnProblem> {
        let name = self.tagged_profile_name(region, resource_arn)?;
        self.profile(region, name)
            .filter(|profile| profile.arn == resource_arn)
            .ok_or(TaggingArnProblem::NoSuchProfile)
    }

    /// [`Self::profile_for_tagging`], for a call that CHANGES the profile's tags.
    pub fn profile_for_tagging_mut(
        &mut self,
        region: &str,
        resource_arn: &str,
    ) -> Result<&mut SigningProfileRecord, TaggingArnProblem> {
        let key = profile_key(region, self.tagged_profile_name(region, resource_arn)?);
        self.profiles
            .get_mut(&key)
            .filter(|profile| profile.arn == resource_arn)
            .ok_or(TaggingArnProblem::NoSuchProfile)
    }

    /// The profile name a tagging call's `resourceArn` names, for a call `region`
    /// could serve at all.
    fn tagged_profile_name<'a>(
        &self,
        region: &str,
        resource_arn: &'a str,
    ) -> Result<&'a str, TaggingArnProblem> {
        let (arn_region, name) =
            profile_region_and_name_in_arn(resource_arn).ok_or(TaggingArnProblem::Malformed)?;
        if arn_region != region {
            return Err(TaggingArnProblem::NoSuchProfile);
        }
        Ok(name)
    }
}

/// The id namespace signing-job ids are minted in.
const JOB_ID_NAMESPACE: u64 = 0;

/// The id namespace profile tokens are minted in, so no token ever collides with
/// a job id.
const PROFILE_TOKEN_NAMESPACE: u64 = 1;

/// The id namespace permission-policy revision ids are minted in, so no revision
/// id ever collides with a job id or a profile token.
const PERMISSION_REVISION_NAMESPACE: u64 = 2;

/// `(namespace, counter)` rendered as the 32 hex digits of a UUID, in UUID form.
///
/// AWS mints both job ids and profile tokens as UUIDs, and a client is entitled
/// to parse them as one; the counter is what makes each unique.
fn uuid_shaped(namespace: u64, counter: u64) -> String {
    let hex = format!("{namespace:016x}{counter:016x}");
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Ten base-62 digits of `value`, the fixed-width form `ProfileVersion`'s
/// `@length(10, 10)` and `^[a-zA-Z0-9]{10}$` require.
fn base62_10(value: u64) -> String {
    // The alphabet `^[a-zA-Z0-9]{10}$` admits, in digit order.
    const ALPHABET: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
    let mut digits = [b'0'; 10];
    let mut rest = value;
    for slot in digits.iter_mut().rev() {
        *slot = ALPHABET[(rest % 62) as usize];
        rest /= 62;
    }
    String::from_utf8(digits.to_vec()).expect("the alphabet is ASCII")
}

/// `map` as the wire `HashMap`, or `None` when it is empty.
fn non_empty_map(map: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if map.is_empty() {
        return None;
    }
    Some(
        map.iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// The ARN of the signing profile `profile_name` names.
///
/// Signer's profile ARNs carry a leading `/` in the resource portion:
/// `arn:aws:signer:<region>:<account>:/signing-profiles/<name>`.
pub fn profile_arn(profile_name: &str, region: &str) -> String {
    ::engine::emulator_arn(
        "signer",
        region,
        &format!("/signing-profiles/{profile_name}"),
    )
}

/// The resource portion every signing-profile ARN carries before the name — see
/// [`profile_arn`] for the whole form.
const PROFILE_ARN_RESOURCE_PREFIX: &str = "/signing-profiles/";

/// The profile name inside `arn` — [`profile_region_and_name_in_arn`] without
/// the region, for a caller that only needs the name.
pub fn profile_name_in_arn(arn: &str) -> Option<&str> {
    profile_region_and_name_in_arn(arn).map(|(_, name)| name)
}

/// The region and the profile name inside `arn`, when `arn` is a well-formed AWS
/// Signer signing-profile ARN —
/// `arn:<partition>:signer:<region>:<account>:/signing-profiles/<name>`, the form
/// [`profile_arn`] builds.
///
/// Both fields are part of a profile's identity: Signer is regional, so the
/// region an ARN names is the region whose profile it names (see [`ProfileKey`]).
/// `None` for anything else: an ARN of another service, one whose resource is not
/// a signing profile, one carrying a profile VERSION after the name (tags belong
/// to the profile, not to one of its versions), or a name outside
/// [`is_profile_name`]'s alphabet. It is a judgement on the STRING alone, so it
/// says nothing about whether such a profile exists.
pub fn profile_region_and_name_in_arn(arn: &str) -> Option<(&str, &str)> {
    let (region, resource) = signer_region_and_resource(arn)?;
    let name = resource.strip_prefix(PROFILE_ARN_RESOURCE_PREFIX)?;
    is_profile_name(name).then_some((region, name))
}

/// The region, profile name and profile version inside `arn`, when `arn` is a
/// well-formed signing-profile VERSION ARN — a profile ARN with the version
/// after it, `arn:<partition>:signer:<region>:<account>:/signing-profiles/<name>/<version>`,
/// which is the form [`SigningProfileRecord::version_arn`] builds and
/// `PutSigningProfile` reports as `profileVersionArn`.
///
/// `None` for anything else: an ARN of another service or another resource, one
/// carrying no version after the name, a name outside [`is_profile_name`]'s
/// alphabet or a version outside [`is_profile_version`]'s. Like
/// [`profile_region_and_name_in_arn`] it is a judgement on the STRING alone, so
/// it says nothing about whether such a version exists.
pub fn profile_version_in_arn(arn: &str) -> Option<(&str, &str, &str)> {
    let (region, resource) = signer_region_and_resource(arn)?;
    let (name, version) = resource
        .strip_prefix(PROFILE_ARN_RESOURCE_PREFIX)?
        .split_once('/')?;
    (is_profile_name(name) && is_profile_version(version)).then_some((region, name, version))
}

/// The resource portion every signing-job ARN carries before the job id — see
/// [`job_arn`] for the whole form.
const JOB_ARN_RESOURCE_PREFIX: &str = "/signing-jobs/";

/// The ARN of the signing job `job_id` names, as served by `region`.
///
/// Signer's job ARNs carry the leading `/` in the resource portion its profile
/// ARNs carry ([`profile_arn`]):
/// `arn:aws:signer:<region>:<account>:/signing-jobs/<jobId>`. Every modelled
/// read of a job names it by `jobId`, so this is what a `jobArn` a request
/// carries is resolved against.
pub fn job_arn(job_id: &str, region: &str) -> String {
    ::engine::emulator_arn(
        "signer",
        region,
        &format!("{JOB_ARN_RESOURCE_PREFIX}{job_id}"),
    )
}

/// The region and the job id inside `arn`, when `arn` is a well-formed
/// signing-job ARN — the form [`job_arn`] builds. `None` for anything else: an
/// ARN of another service, one whose resource is not a signing job, and one
/// carrying no id or a further `/`-separated part after it.
pub fn job_region_and_id_in_arn(arn: &str) -> Option<(&str, &str)> {
    let (region, resource) = signer_region_and_resource(arn)?;
    let job_id = resource.strip_prefix(JOB_ARN_RESOURCE_PREFIX)?;
    (!job_id.is_empty() && !job_id.contains('/')).then_some((region, job_id))
}

/// Whether `arn` is a well-formed AWS Signer ARN at all — whatever resource it
/// names, and whether or not such a resource exists.
///
/// For a member a request carries an ARN in: a value that is not one is a value
/// this service cannot resolve to any resource of its own, while a Signer ARN
/// naming nothing stored is a resource that merely does not exist here.
pub fn is_signer_arn(arn: &str) -> bool {
    signer_region_and_resource(arn).is_some()
}

/// The region and the resource portion of `arn`, when `arn` is a well-formed AWS
/// Signer ARN: `arn:<partition>:signer:<region>:<account>:<resource>`.
///
/// The ONE place this service takes an ARN apart, so every resource an ARN can
/// name — a signing profile, one of its versions, a signing job — is read out of
/// the same fields: a non-empty partition, the `signer` service, the region the
/// resource lives in (Signer is regional, see [`ProfileKey`]) and a non-empty
/// account. The resource is whatever follows, for the caller to judge.
fn signer_region_and_resource(arn: &str) -> Option<(&str, &str)> {
    // `arn : partition : service : region : account : resource`, and the resource
    // is the remainder because it carries `:` of its own in other ARN forms.
    let mut fields = arn.splitn(6, ':');
    if fields.next()? != "arn" {
        return None;
    }
    if fields.next()?.is_empty() {
        return None;
    }
    if fields.next()? != "signer" {
        return None;
    }
    // Region and account: present, and non-empty (Signer is a regional service
    // and its resources belong to one account).
    let region = fields.next()?;
    if region.is_empty() || fields.next()?.is_empty() {
        return None;
    }
    Some((region, fields.next()?))
}

/// Why a tagging call could not reach a signing profile through its
/// `resourceArn`.
///
/// The two cases answer DIFFERENT modelled errors, which is why they are told
/// apart: a label that is not a signing-profile ARN at all is a bad request
/// (`BadRequestException`, "The request contains invalid parameters for the ARN or
/// tags"), while a well-formed ARN naming no stored profile is a missing resource
/// (`NotFoundException`, "The signing profile was not found").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggingArnProblem {
    /// `resourceArn` is not a signing-profile ARN.
    Malformed,
    /// A signing-profile ARN, but this account holds no such profile in that
    /// region.
    NoSuchProfile,
}

impl TaggingArnProblem {
    /// The `code` a client branches on — the condition that was refused.
    pub fn error_code(self) -> &'static str {
        match self {
            Self::Malformed => ERROR_CODE_BAD_REQUEST,
            Self::NoSuchProfile => ERROR_CODE_PROFILE_NOT_FOUND,
        }
    }

    /// The prose beside the code, built from the request's own ARN.
    ///
    /// `resourceArn` is a bare string in the model, so the value is quoted
    /// through the one ceiling this crate states for a request value a clause
    /// carries ([`stated_value`]).
    pub fn message(self, resource_arn: &str) -> String {
        match self {
            // The label was not an ARN at all, which is a statement about its
            // SYNTAX: the clause says so and carries none of the value, so a
            // label nothing parsed is never reported back to the caller.
            Self::Malformed => "Invalid input resource arn: bad syntax".to_string(),
            Self::NoSuchProfile => {
                format!(
                    "profile with arn {} does not exists",
                    stated_value(resource_arn)
                )
            }
        }
    }
}

/// The modelled refusal one of this service's tagging operations answers a
/// [`TaggingArnProblem`] with, in that operation's OWN error channel.
///
/// The branch from a problem to the shape that answers it is stated HERE, once,
/// beside the enum that tells the two cases apart and owns their `code` and prose:
/// `Malformed` answers `BadRequestException` (400) and `NoSuchProfile` answers
/// `NotFoundException` (404), so which condition answers which shape is one fact in
/// one place for all three operations bound to the tagging route.
///
/// Each of those operations has its OWN generated error enum, so the mapping cannot
/// be a function over one type; it is this macro instead, expanded once per handler
/// with `$modeled` naming that handler's `{Op}ErrorModeled`. Everything in the body
/// — both members of both shapes — comes off the problem itself, so every expansion
/// answers the same status, code and message for the same `resourceArn`.
#[macro_export]
macro_rules! tagging_arn_refusal {
    ($modeled:ident, $problem:expr, $resource_arn:expr) => {{
        let problem: $crate::state::TaggingArnProblem = $problem;
        let message = ::std::option::Option::Some(problem.message($resource_arn));
        let code = ::std::option::Option::Some(problem.error_code().to_string());
        let modeled = match problem {
            $crate::state::TaggingArnProblem::Malformed => $modeled::BadRequestException(
                $crate::ops::types::BadRequestException { message, code },
            ),
            $crate::state::TaggingArnProblem::NoSuchProfile => $modeled::NotFoundException(
                $crate::ops::types::NotFoundException { message, code },
            ),
        };
        modeled.into()
    }};
}

/// True when `character` is one the modelled tag-key character set admits.
///
/// `TagKey`'s pattern is `^(?!aws:)[a-zA-Z+-=._:/]+$`
/// (`smithy_query(query="shape", shape="TagKey")`). Its character class is the
/// ASCII letters, the inclusive range from `+` to `=` — which covers the digits
/// and `+ , - . / : ; < =` — and `_`; the class also spells `.`, `:` and `/` out
/// again, all three of which that range already holds. Nothing else is admitted:
/// not a space, not `!`, `#`, `$`, `%`, `&`, `'`, `(`, `)`, `*`, `?`, `@`, `[`,
/// `\`, `]`, `^`, a backtick, `{`, `|`, `}`, `~`, and no character outside ASCII.
pub fn tag_key_character_admitted(character: char) -> bool {
    character.is_ascii_alphabetic() || ('+'..='=').contains(&character) || character == '_'
}

/// Why a tag key is not one the modelled `TagKey` shape admits.
///
/// Each variant is one half of that shape's constraints: its `@length(min: 1,
/// max: 128)` and the two halves of its `@pattern` — the negative lookahead that
/// excludes AWS's reserved namespace, and the character class that states the
/// alphabet (see [`tag_key_character_admitted`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagKeyProblem {
    /// Outside `TagKey`'s modelled length bounds, carrying this many characters.
    Length(usize),
    /// Inside AWS's reserved namespace ([`RESERVED_TAG_KEY_PREFIX`]).
    Reserved,
    /// Carries a character the modelled alphabet does not admit.
    Character(char),
}

/// Why `key` is not a key the modelled `TagKey` admits, or `None` when it is one.
///
/// The length is judged first so the prose for either later condition can echo a
/// key the model's own bound already limits.
pub fn tag_key_problem(key: &str) -> Option<TagKeyProblem> {
    let length = key.chars().count();
    if !(TAG_KEY_MIN_LENGTH..=TAG_KEY_MAX_LENGTH).contains(&length) {
        return Some(TagKeyProblem::Length(length));
    }
    if key.starts_with(RESERVED_TAG_KEY_PREFIX) {
        return Some(TagKeyProblem::Reserved);
    }
    key.chars()
        .find(|character| !tag_key_character_admitted(*character))
        .map(TagKeyProblem::Character)
}

/// The first key of `keys` the modelled `TagKey` does not admit, with the reason,
/// or `None` when every one of them is a key it admits.
///
/// Every operation that takes tag keys judges them through this, so one rule
/// decides what any of them may write: the keys a read of a signing profile can
/// report are exactly the keys `TagKey` admits.
pub fn invalid_tag_key<'a, I>(keys: I) -> Option<(&'a str, TagKeyProblem)>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter()
        .find_map(|key| tag_key_problem(key).map(|problem| (key, problem)))
}

/// The refusal prose for a tag key the modelled `TagKey` does not admit: which
/// key, and which of its constraints the key missed.
pub fn invalid_tag_key_message(key: &str, problem: TagKeyProblem) -> String {
    match problem {
        TagKeyProblem::Length(length) => format!(
            "a tag key is between {TAG_KEY_MIN_LENGTH} and {TAG_KEY_MAX_LENGTH} characters long; \
             this request carries one of {length}"
        ),
        TagKeyProblem::Reserved => {
            format!("tag key {key} is in the reserved prefix {RESERVED_TAG_KEY_PREFIX}")
        }
        TagKeyProblem::Character(character) => format!(
            "tag key {key} carries {character:?}, which a tag key may not: a tag key is made of \
             letters, digits and the characters +,-./:;<=_"
        ),
    }
}

/// The refusal prose for a tag write that would leave a profile holding more
/// tags than [`MAX_TAGS_PER_PROFILE`].
pub fn too_many_tags_message(resulting: usize) -> String {
    format!(
        "a signing profile holds at most {MAX_TAGS_PER_PROFILE} tags; this request would leave \
         it holding {resulting}"
    )
}

/// The refusal prose for a cancel of a profile whose signatures have already
/// been revoked.
///
/// Worded like the tagging refusal below it ([`tagging_refused_message`]): the
/// profile, the call it cannot admit, and the condition of the profile that
/// forbids it.
pub fn cancel_refused_message(profile_name: &str) -> String {
    format!(
        "Signing Profile {profile_name} cannot be canceled because it has been revoked. \
         A revoked profile cannot be canceled."
    )
}

/// The refusal prose for a tagging call on a profile whose status forbids it.
pub fn tagging_refused_message(profile_name: &str, status: &str) -> String {
    format!(
        "Signing Profile {profile_name} cannot be tagged because it is {status}. \
         A {status} profile cannot be tagged."
    )
}

/// Where Signer saved the signed code image: the destination bucket, under the
/// destination prefix, named for the job that produced it.
pub fn signed_object_of(destination: &Destination, job_id: &str) -> SignedObject {
    let s3 = destination.s3.as_ref();
    let prefix = s3.and_then(|s3| s3.prefix.clone()).unwrap_or_default();
    SignedObject {
        s3: Some(S3SignedObject {
            bucket_name: s3.and_then(|s3| s3.bucket_name.clone()),
            key: Some(format!("{prefix}{job_id}")),
        }),
    }
}

/// The `category` the IoT device platforms report: the IoT/FreeRTOS platforms
/// whose signatures a device verifies.
///
/// `Category` is modelled as an enum with exactly ONE value, `AWSIoT`
/// (`smithy_query(query="shape", shape="Category")`, mirrored in
/// [`crate::ops::types::CATEGORY_VALUES`]) — but the recorded
/// `GetSigningPlatform` answers [`PLATFORM_CATEGORY_AWS`] for the AWS Lambda
/// platform, so the enum does not name every category the service reports and the
/// category is a per-platform fact of the catalog rather than a constant.
pub const PLATFORM_CATEGORY_AWS_IOT: &str = "AWSIoT";

/// The `category` AWS's own (non-device) platforms report — Lambda code signing
/// and container images.
///
/// Recorded behaviour: the `GetSigningPlatform` recording for the AWS Lambda
/// platform reports `AWS`, a value this model's `Category` enum predates (see
/// [`PLATFORM_CATEGORY_AWS_IOT`]).
pub const PLATFORM_CATEGORY_AWS: &str = "AWS";

/// Which of AWS Signer's two signing workflows a platform's profiles are signed
/// through.
///
/// A signing platform belongs to exactly one of them, and the signing operation
/// of the OTHER workflow is not available for its profiles. The model states the
/// split per action: `AddProfilePermissionRequest$action` documents
/// `signer:StartSigningJob` as "not supported for container image workflows" and,
/// symmetrically, `signer:SignPayload` as "not supported for AWS Lambda
/// workflows"; `StartSigningJob`'s own documentation describes the S3
/// source/destination flow only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningWorkflow {
    /// The S3 code-signing-job workflow: the caller names an S3 source object and
    /// a destination bucket and `StartSigningJob` signs the code into it.
    CodeSigningJob,
    /// The container-image (Notation/OCI) workflow: the caller hands over the
    /// payload digest and `SignPayload` answers with the signature envelope, so
    /// no object is read from or written to S3.
    ContainerImage,
}

impl SigningWorkflow {
    /// The operation that signs through this workflow — the name a refusal states
    /// so a client knows which of the two it should have called.
    pub fn signing_operation(self) -> &'static str {
        match self {
            Self::CodeSigningJob => "StartSigningJob",
            Self::ContainerImage => "SignPayload",
        }
    }
}

/// One signing platform this emulator publishes.
///
/// Every member is a fact about the PLATFORM — none of it comes from a request —
/// so the readers of the catalog ([`signing_platform`] and the handlers behind
/// `GetSigningPlatform`) all report the same answer for a platform id.
#[derive(Debug, Clone, Copy)]
pub struct SigningPlatformEntry {
    /// The `platformId` a `PutSigningProfile` names.
    pub platform_id: &'static str,
    /// The `displayName` AWS Signer publishes for it, or `None` for a platform it
    /// publishes none for (the recorded profile responses report no
    /// `platformDisplayName` for the container-image platform).
    pub display_name: Option<&'static str>,
    /// The `category` the platform is published in — one of
    /// [`PLATFORM_CATEGORY_AWS_IOT`] / [`PLATFORM_CATEGORY_AWS`], which is where
    /// the per-platform split is explained.
    pub category: &'static str,
    /// The `partner` entity that uses the platform — the leading segment(s) of
    /// [`Self::platform_id`], the rest of which is the platform's `target` (see
    /// [`Self::target`]).
    pub partner: &'static str,
    /// The `EncryptionAlgorithm`s a signature on this platform may be produced
    /// under, MOST PREFERRED FIRST: the head is the platform's `defaultValue`
    /// and the whole slice is its `allowedValues`, so a default outside the
    /// allowed set is not expressible.
    pub encryption_algorithms: &'static [&'static str],
    /// The `HashAlgorithm`s, same convention: head is the default.
    pub hash_algorithms: &'static [&'static str],
    /// The `ImageFormat`s the platform's signing image may take, head first: the
    /// head is `defaultFormat` and the slice is `supportedFormats`.
    pub image_formats: &'static [&'static str],
    /// The largest payload (in MB) the platform signs.
    pub max_size_in_mb: i32,
    /// Whether signatures produced for the platform can be revoked.
    pub revocation_supported: bool,
    /// Whether a profile on this platform must name the ACM certificate its
    /// signatures are issued under (`signingMaterial`).
    ///
    /// True for a platform the CALLER supplies the signing certificate for, and
    /// false for one AWS Signer issues the certificate for itself — see the note
    /// on [`SIGNING_PLATFORM_CATALOG`] for which published platforms are which.
    pub requires_signing_material: bool,
    /// Which signing workflow a profile on this platform is signed through, and
    /// therefore which signing operation its profiles accept (see
    /// [`SigningWorkflow`]).
    pub workflow: SigningWorkflow,
}

impl SigningPlatformEntry {
    /// The `target` validation template the platform signs for: the part of
    /// [`Self::platform_id`] after the [`Self::partner`] that owns it.
    ///
    /// Not a per-entry fact but a decomposition of the id: the recorded
    /// `GetSigningPlatform` reports a platform whose id is `AWSLambda-SHA384-ECDSA`
    /// with `partner` `AWSLambda` and `target` `SHA384-ECDSA`, i.e. the id is the
    /// partner and the target joined by `-`. Deriving it here is what keeps the
    /// three members from contradicting each other.
    pub fn target(&self) -> &'static str {
        self.platform_id
            .strip_prefix(self.partner)
            .and_then(|rest| rest.strip_prefix('-'))
            .unwrap_or(self.platform_id)
    }

    /// The platform's `signingConfiguration`: which algorithms a signature on it
    /// may use, and which one it uses when the profile overrides nothing.
    ///
    /// Built from [`Self::encryption_algorithms`] / [`Self::hash_algorithms`], so
    /// `defaultValue` is always one of `allowedValues`.
    pub fn signing_configuration(&self) -> SigningConfiguration {
        SigningConfiguration {
            encryption_algorithm_options: EncryptionAlgorithmOptions {
                allowed_values: to_owned_values(self.encryption_algorithms),
                default_value: default_value(self.encryption_algorithms),
            },
            hash_algorithm_options: HashAlgorithmOptions {
                allowed_values: to_owned_values(self.hash_algorithms),
                default_value: default_value(self.hash_algorithms),
            },
        }
    }

    /// The platform's `signingImageFormat`, with `defaultFormat` always one of
    /// `supportedFormats` ([`Self::image_formats`]).
    pub fn signing_image_format(&self) -> SigningImageFormat {
        SigningImageFormat {
            supported_formats: to_owned_values(self.image_formats),
            default_format: default_value(self.image_formats),
        }
    }

    /// The `SigningPlatform` summary a platform listing reports for this entry.
    ///
    /// Every member is read off the SAME catalog entry `GetSigningPlatform`
    /// reports, so what a listing says about a platform and what a get says
    /// about it cannot drift apart.
    pub fn to_signing_platform(&self) -> SigningPlatform {
        SigningPlatform {
            platform_id: Some(self.platform_id.to_string()),
            display_name: self.display_name.map(str::to_string),
            partner: Some(self.partner.to_string()),
            target: Some(self.target().to_string()),
            category: Some(self.category.to_string()),
            signing_configuration: Some(self.signing_configuration()),
            signing_image_format: Some(self.signing_image_format()),
            max_size_in_mb: self.max_size_in_mb,
            revocation_supported: self.revocation_supported,
        }
    }
}

/// `values` as the owned strings a modelled list member holds.
fn to_owned_values(values: &[&'static str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}

/// The head of `values` — the catalog's most-preferred-first convention — or the
/// empty string for a platform that lists none.
fn default_value(values: &[&'static str]) -> String {
    values.first().copied().unwrap_or_default().to_string()
}

/// The signing platforms this emulator publishes: the catalog
/// `GetSigningPlatform` and `ListSigningPlatforms` serve, and the one the
/// profile/job slice resolves a platform's display name and signing-material
/// rule against.
///
/// Source: the code-signing platforms AWS Signer publishes — AWS IoT Device
/// Management, AWS Lambda, the two FreeRTOS platforms and the Notation
/// container-image platform — with the `platformId`, `displayName`, `partner`,
/// algorithm options, image formats, `maxSizeInMB` and `revocationSupported` AWS
/// reports for each; `target` is not stated per entry but derived from the id
/// (see [`SigningPlatformEntry::target`]). A platform id spells out the
/// algorithms it signs under (`…-SHA384-ECDSA`), and
/// `hashAlgorithmOptions.allowedValues` is what tells a client which algorithm it
/// may override to, so a platform that signs under SHA384 reports SHA384 even
/// though this model's `HashAlgorithm` enum
/// ([`crate::ops::types::HASH_ALGORITHM_VALUES`]) predates it and cannot name it.
///
/// `requires_signing_material` is a SEPARATE fact from publishing a platform,
/// stated per entry rather than inferred from membership: a profile is signed
/// under a certificate, and whether the CALLER supplies that certificate is a
/// property of the platform. The device-signing platforms (AWS IoT Device
/// Management and the two FreeRTOS ones) sign with a certificate the caller
/// brings in AWS Certificate Manager, so a profile on them must name it and a
/// create that omits it is refused ("signing material cannot be null."); the
/// platforms AWS Signer issues the certificate for — AWS Lambda code signing and
/// the container-image platform — take none, and the recorded `PutSigningProfile`
/// for each of those two accepts a profile created with no `signingMaterial`.
///
/// [`SigningPlatformEntry::workflow`] is stated per entry for the same reason:
/// the code-signing platforms are signed through the S3 signing-job workflow and
/// the Notation container-image platform through the payload workflow, which is
/// the split the model's own action documentation states (see
/// [`SigningWorkflow`]).
///
/// The catalog is the WHOLE set of platforms a profile can be created on: an id
/// outside it names no platform, so there is no entry to read a signing
/// configuration, a display name or a signing-material rule off, and the create
/// is refused rather than guessed at ([`signing_platform`]).
pub const SIGNING_PLATFORM_CATALOG: &[SigningPlatformEntry] = &[
    SigningPlatformEntry {
        platform_id: "AWSIoTDeviceManagement-SHA256-ECDSA",
        display_name: Some("AWS IoT Device Management SHA256-ECDSA"),
        category: PLATFORM_CATEGORY_AWS_IOT,
        partner: "AWSIoTDeviceManagement",
        encryption_algorithms: &["ECDSA"],
        hash_algorithms: &["SHA256"],
        image_formats: &["JSONDetached"],
        max_size_in_mb: 2048,
        revocation_supported: true,
        requires_signing_material: true,
        workflow: SigningWorkflow::CodeSigningJob,
    },
    SigningPlatformEntry {
        platform_id: "AWSLambda-SHA384-ECDSA",
        display_name: Some("AWS Lambda"),
        category: PLATFORM_CATEGORY_AWS,
        partner: "AWSLambda",
        encryption_algorithms: &["ECDSA"],
        hash_algorithms: &["SHA384"],
        image_formats: &["JSONDetached"],
        max_size_in_mb: 250,
        revocation_supported: true,
        requires_signing_material: false,
        workflow: SigningWorkflow::CodeSigningJob,
    },
    SigningPlatformEntry {
        platform_id: "AmazonFreeRTOS-Default",
        display_name: Some("AmazonFreeRTOS SHA256-ECDSA"),
        category: PLATFORM_CATEGORY_AWS_IOT,
        partner: "AmazonFreeRTOS",
        encryption_algorithms: &["ECDSA"],
        hash_algorithms: &["SHA256"],
        image_formats: &["JSON"],
        max_size_in_mb: 16,
        revocation_supported: false,
        requires_signing_material: true,
        workflow: SigningWorkflow::CodeSigningJob,
    },
    SigningPlatformEntry {
        platform_id: "AmazonFreeRTOS-TI-CC3220SF",
        display_name: Some("AmazonFreeRTOS SHA1-RSA CC3220SF-Format"),
        category: PLATFORM_CATEGORY_AWS_IOT,
        partner: "AmazonFreeRTOS",
        encryption_algorithms: &["RSA"],
        hash_algorithms: &["SHA1"],
        image_formats: &["JSON"],
        max_size_in_mb: 16,
        revocation_supported: false,
        requires_signing_material: true,
        workflow: SigningWorkflow::CodeSigningJob,
    },
    SigningPlatformEntry {
        platform_id: "Notation-OCI-SHA384-ECDSA",
        display_name: None,
        category: PLATFORM_CATEGORY_AWS,
        partner: "Notation",
        encryption_algorithms: &["ECDSA"],
        hash_algorithms: &["SHA384"],
        image_formats: &["JSONDetached"],
        max_size_in_mb: 16,
        revocation_supported: true,
        requires_signing_material: false,
        workflow: SigningWorkflow::ContainerImage,
    },
];

/// The catalog entry for `platform_id`, or `None` for a platform this emulator
/// publishes no entry for.
///
/// The ONE place a `platformId` carried by a request is resolved: every fact a
/// handler needs about a platform — its display name, its signing configuration,
/// whether it requires signing material, whether it supports revocation — is read
/// off the entry this answers, so an id that resolves to nothing is refused
/// instead of being treated as a platform with unknown properties.
pub fn signing_platform(platform_id: &str) -> Option<&'static SigningPlatformEntry> {
    SIGNING_PLATFORM_CATALOG
        .iter()
        .find(|entry| entry.platform_id == platform_id)
}

/// The refusal prose for a `platformId` this emulator publishes no platform for.
///
/// Says the id was WELL FORMED and unknown — a platform another endpoint may
/// publish, or one that was withdrawn. An id that is not a platform id at all is
/// a different answer ([`platform_id_pattern_refusal`]).
///
/// `PlatformId` is a bare string in the model, so a value the published
/// character class admits reaches here at any length: the id is quoted through
/// the one ceiling this crate states for a request value a clause carries
/// ([`stated_value`]), the same ceiling the syntax refusal of this very member
/// applies.
pub fn platform_not_found_message(platform_id: &str) -> String {
    format!(
        "Signing platform not found for ID {}",
        stated_value(platform_id)
    )
}

/// The refusal prose for a `jobId` no signing job is held under, as
/// `RevokeSignature` words it.
///
/// Per OPERATION rather than per condition: the recorded 404 of the job READER
/// (`DescribeSigningJob`) words the same condition its own way, so each states
/// the sentence its own answer carries. `JobId` is a bare string in the model, so
/// the id is quoted through the one ceiling this crate states for a request value
/// a clause carries ([`stated_value`]).
pub fn job_not_found_message(job_id: &str) -> String {
    format!(
        "Signing Job with id {} does not exist.",
        stated_value(job_id)
    )
}

/// The refusal prose for a VERSION of a signing profile this endpoint cannot
/// resolve.
///
/// One sentence for both halves of the addressed resource: a request that
/// addresses a version names a profile AND a version, and either half can be the
/// one that did not resolve — the profile may be unknown, or the profile may
/// never have had the version — so the clause states both, and a caller reading
/// it can decide whether to re-create the profile or re-read its versions.
///
/// Shared by every operation that resolves a version of a profile, so the one
/// condition reads the same whichever one refused it. Both values come from the
/// request (`profileName` is the path label, `profileVersion` the member), and
/// both are bounded by the model (`@length 2..64` and `@length 10..10`), so the
/// clause is already request-independent in size and needs no further ceiling.
pub fn profile_version_not_found_message(profile_name: &str, profile_version: &str) -> String {
    format!(
        "Signing Profile with name {profile_name} and version {profile_version} does not exist."
    )
}

/// The character class a `platformId` carried by `GET /revocations` is published
/// to fall inside.
///
/// Source: the `@pattern` the model of the signer DATA PLANE — which publishes
/// the contract of that route — declares on `PlatformId`
/// (`smithy_query(query="shape",shape="PlatformId",model_path="services/\
/// service-com-amazonaws-signerdata-signerdataplane/smithy-model/\
/// com-amazonaws-signerdata-signerdataplane.json")`). This service serves that
/// route (it holds the revocation records that answer it) while its own copy of
/// the operation omits the trait, so the published expression is stated here —
/// once, because a refusal quotes the expression a value had to satisfy and
/// [`satisfies_platform_id_pattern`] decides against the same one.
pub const REVOCATION_PLATFORM_ID_PATTERN: &str = "^[a-zA-Z0-9_-]+$";

/// Whether `value` falls inside [`REVOCATION_PLATFORM_ID_PATTERN`]: at least one
/// character, every one of them an ASCII letter, an ASCII digit, `_` or `-`.
///
/// The expression is anchored at both ends, so satisfying it is a property of
/// the WHOLE value: one character outside the class — a space, a `.`, a `/`, a
/// `!`, anything non-ASCII — puts the value outside it, and so does the empty
/// string.
fn satisfies_platform_id_pattern(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// The longest rejected value a clause states in full.
///
/// The value between the quotes is the caller's OWN bytes — AWS builds the
/// clause from the request, so a caller comparing the echo against what it sent
/// must see it whole. A value with no ceiling at all would let one oversized
/// query parameter become an equally large error body, so the ceiling is set
/// where the shared refusal-wording module sets the largest echo it restores
/// into a clause (16 KiB): 64x the longest identifier any member of this service
/// declares, so every value a client plausibly sends — a long id, an over-long
/// ARN — is quoted whole, and only request-sized content is shortened.
const MAX_STATED_VALUE: usize = 16 * 1024;

/// `value` as a clause states it: itself when it is within
/// [`MAX_STATED_VALUE`], otherwise the prefix that fits — cut on a character
/// boundary, never mid-UTF-8 — with the whole length stated after it, which is
/// how the framework shortens a value too large for a clause it renders itself.
///
/// Rendering only: the value is checked against the published class UNELIDED
/// ([`satisfies_platform_id_pattern`]), because a decision taken on a prefix
/// would refuse a long value the class admits.
///
/// The ONE place this crate quotes a request value whose member the model puts
/// no length on — a `platformId`, a tagging `resourceArn`, a `jobId`, a
/// permission's `statementId`, `revisionId` or `action` — so every clause of
/// every operation carries the same ceiling and no refusal is sized by the
/// request. A member the model DOES bound (an `Arn`, a `ProfileName`, a
/// `ProfileVersion`, an `AccountId`, a `TagKey`, an enumerated value) is already
/// bounded when the boundary validator admits it, and is quoted as it arrived.
pub fn stated_value(value: &str) -> String {
    if value.len() <= MAX_STATED_VALUE {
        return value.to_string();
    }
    // `end` starts inside the value (its length is past the bound) and only
    // walks down, so the slice below is always in range and on a boundary.
    let mut end = MAX_STATED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}... [{} bytes]", &value[..end], value.len())
}

/// How a `platformId` outside the published character class is refused, or
/// `None` when it falls inside it.
///
/// A value outside a published constraint is refused for the SYNTAX it violated,
/// which is a different answer from a catalog lookup's
/// ([`platform_not_found_message`]): a value that is not a platform id at all
/// names no platform, so it is never reported as a platform that does not exist.
/// It is therefore decided before anything the value might name is resolved.
///
/// The sentence is the one the boundary validator writes for every constraint it
/// refuses — the clause [`::aws_smithy_schema_server::Violation::message`]
/// renders from the member path and the violated requirement, inside
/// [`::aws_smithy_schema_server::envelope`] — so a constraint this service has
/// to check itself is worded exactly like one the model carries. `member` is the
/// Smithy member name the value arrived on, put through the framework's own path
/// builder, so every member held to the class is named one way.
pub fn platform_id_pattern_refusal(member: &str, value: &str) -> Option<String> {
    if satisfies_platform_id_pattern(value) {
        return None;
    }
    ::aws_smithy_schema_server::envelope(&[::aws_smithy_schema_server::Violation {
        path: ::aws_smithy_schema_server::constraints::path::member(member),
        kind: ::aws_smithy_schema_server::ViolationKind::Pattern(
            REVOCATION_PLATFORM_ID_PATTERN.to_string(),
        ),
        value: Some(stated_value(value)),
        sensitive: false,
    }])
}

/// Which `overrides.signingConfiguration` member an override refusal is about.
///
/// A platform publishes one option set per member
/// ([`SigningPlatformEntry::signing_configuration`]), so the member is what says
/// which set the overridden value had to fall inside.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SigningConfigurationMember {
    /// `SigningConfigurationOverrides$hashAlgorithm`, held to the platform's
    /// `signingConfiguration.hashAlgorithmOptions.allowedValues`.
    HashAlgorithm,
    /// `SigningConfigurationOverrides$encryptionAlgorithm`, held to the
    /// platform's `signingConfiguration.encryptionAlgorithmOptions.allowedValues`.
    EncryptionAlgorithm,
}

impl SigningConfigurationMember {
    /// The member's name as the model spells it on the wire — what a refusal
    /// names so a client knows which override it has to change.
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::HashAlgorithm => "hashAlgorithm",
            Self::EncryptionAlgorithm => "encryptionAlgorithm",
        }
    }

    /// The `allowedValues` `entry` publishes for this member.
    pub fn allowed_values(self, entry: &SigningPlatformEntry) -> &'static [&'static str] {
        match self {
            Self::HashAlgorithm => entry.hash_algorithms,
            Self::EncryptionAlgorithm => entry.encryption_algorithms,
        }
    }
}

/// The first `overrides.signingConfiguration` member whose value is not one the
/// platform publishes as allowed, with that value — or `None` when every override
/// the request carries falls inside the platform's published options.
///
/// An override selects among the algorithms the PLATFORM signs under, which is
/// exactly what its `allowedValues` publish; a value outside them names a
/// configuration the platform publishes none of, so nothing could be signed
/// under it. A member the request leaves out is not an override at all and takes
/// the platform's `defaultValue`.
pub fn unsupported_signing_configuration_override<'a>(
    entry: &SigningPlatformEntry,
    overrides: Option<&'a SigningPlatformOverrides>,
) -> Option<(SigningConfigurationMember, &'a str)> {
    let configuration = overrides.and_then(|overrides| overrides.signing_configuration.as_ref())?;
    [
        (
            SigningConfigurationMember::HashAlgorithm,
            configuration.hash_algorithm.as_deref(),
        ),
        (
            SigningConfigurationMember::EncryptionAlgorithm,
            configuration.encryption_algorithm.as_deref(),
        ),
    ]
    .into_iter()
    .find_map(|(member, value)| {
        let value = value?;
        (!member.allowed_values(entry).contains(&value)).then_some((member, value))
    })
}

/// The refusal prose for an override outside the platform's published options:
/// which member carried it, the value it carried, the platform that publishes no
/// such option, and the values that platform does publish.
pub fn unsupported_override_message(
    entry: &SigningPlatformEntry,
    member: SigningConfigurationMember,
    value: &str,
) -> String {
    let allowed = member.allowed_values(entry).join(", ");
    format!(
        "{} {value} is not supported for platform {}. Supported values: [{allowed}]",
        member.wire_name(),
        entry.platform_id
    )
}

/// The `ImageFormat` values an `overrides.signingImageFormat` may SELECT.
///
/// The member documents its own option set rather than taking the platform's: "when
/// overriding the default signing platform configuration, a customer can select
/// either of two signing formats, `JSONEmbedded` or `JSONDetached`", the third
/// value the enum names being reserved for future use
/// (`smithy_query(query="shape",shape="SigningPlatformOverrides")`). So the
/// modelled `ImageFormat` enum ([`crate::ops::types::IMAGE_FORMAT_VALUES`]) is
/// WIDER than what an override may carry — it also names the format platforms
/// publish but customers may not select — and this is the set an override is held
/// to, on every platform.
pub const SELECTABLE_IMAGE_FORMATS: &[&str] = &["JSONEmbedded", "JSONDetached"];

/// The `overrides.signingImageFormat` the request carries when it is not one a
/// customer may select ([`SELECTABLE_IMAGE_FORMATS`]) — or `None` when the request
/// selects a format a customer may select, or overrides the format not at all.
pub fn unselectable_image_format_override(
    overrides: Option<&SigningPlatformOverrides>,
) -> Option<&str> {
    let format = overrides.and_then(|overrides| overrides.signing_image_format.as_deref())?;
    (!SELECTABLE_IMAGE_FORMATS.contains(&format)).then_some(format)
}

/// The refusal prose for an image-format override a customer may not select: the
/// member that carried it, the value it carried, and the formats it may carry.
///
/// Worded like the signing-configuration refusal above — the member, the value,
/// then the values that are supported — because it is the same kind of refusal: an
/// override naming a value outside the option set that member is held to.
pub fn unselectable_image_format_message(value: &str) -> String {
    format!(
        "signingImageFormat {value} is not supported. Supported values: [{}]",
        SELECTABLE_IMAGE_FORMATS.join(", ")
    )
}

/// Whether a signature produced on `platform_id` can be revoked — the platform's
/// published `revocationSupported` ("Indicates whether revocation is supported
/// for the platform", [`SigningPlatformEntry::revocation_supported`]), which is
/// how a client learns in advance whether revoking one of its signatures is
/// possible at all.
///
/// A platform this emulator publishes NO entry for states nothing either way, so
/// it is not held to lack the capability: only a platform published as not
/// supporting revocation does.
pub fn platform_supports_revocation(platform_id: &str) -> bool {
    signing_platform(platform_id).is_none_or(|entry| entry.revocation_supported)
}

/// The refusal prose for revoking a signature produced on a platform published as
/// not supporting revocation.
pub fn revocation_unsupported_message(platform_id: &str) -> String {
    format!("Revocation is not supported for platform {platform_id}")
}

/// Whether a profile on `platform_id` is signed through `workflow` — the
/// published [`SigningPlatformEntry::workflow`], which is what decides whether
/// that workflow's signing operation is available for the platform's profiles at
/// all.
///
/// A platform this emulator publishes NO entry for states nothing either way, so
/// it is not held to be outside the workflow: only a platform published for the
/// other one is (the same convention as [`platform_supports_revocation`]).
pub fn platform_signs_through(platform_id: &str, workflow: SigningWorkflow) -> bool {
    signing_platform(platform_id).is_none_or(|entry| entry.workflow == workflow)
}

/// The refusal prose for a signing call on a profile whose platform is published
/// for the other workflow: the operation that is not available for it, and the
/// platform that does not sign through it.
pub fn workflow_unsupported_message(workflow: SigningWorkflow, platform_id: &str) -> String {
    format!(
        "{} is not supported for platform {platform_id}",
        workflow.signing_operation()
    )
}

/// True when `value` satisfies `ProfileName`'s `@length(2, 64)` and
/// `^[a-zA-Z0-9_]{2,}$`.
///
/// The boundary validator already refuses a modelled `ProfileName` member, so
/// this is for a name read out of a value the model does not constrain: the
/// profile name a tagging `resourceArn` carries, and the one a decoded
/// pagination token resumes at.
pub fn is_profile_name(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// True when `value` satisfies `ProfileVersion`'s `@length(10, 10)` and
/// `^[a-zA-Z0-9]{10}$` — the form
/// [`ComAmazonawsSignerWallabyserviceState::mint_profile_version`] mints.
///
/// For a version read out of a value the model does not constrain: the one a
/// signing-profile VERSION ARN carries.
pub fn is_profile_version(value: &str) -> bool {
    value.len() == 10 && value.bytes().all(|b| b.is_ascii_alphanumeric())
}

/// True when `value` has the form
/// [`ComAmazonawsSignerWallabyserviceState::mint_job_id`] mints — the 8-4-4-4-12
/// hex digits of a UUID.
///
/// `JobId` is an unconstrained string, so this is for a job id read out of a
/// value nothing else constrains: the one a decoded pagination token resumes at.
pub fn is_job_id(value: &str) -> bool {
    /// How many hex digits each dash-separated group of a UUID carries.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let mut groups = value.split('-');
    GROUPS.iter().all(|digits| {
        groups.next().is_some_and(|group| {
            group.len() == *digits && group.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
    }) && groups.next().is_none()
}

/// True when `value` has the form of a statement id this service stores — the
/// only form `AddProfilePermission` admits, since `statementId` is an
/// unconstrained `String` in the model: anything but blank, an id no later call
/// could name again.
///
/// For a statement id read out of a value nothing else constrains: the one a
/// decoded pagination token resumes at.
pub fn is_statement_id(value: &str) -> bool {
    !value.trim().is_empty()
}

/// Whether `signature_timestamp` falls at or after `effective_from`.
///
/// A modelled `Timestamp` is whole epoch seconds (signed) plus a non-negative
/// sub-second part, so two of them compare as that pair — correct on both sides
/// of the epoch, where arithmetic over an unsigned duration would saturate.
fn at_or_after(
    signature_timestamp: &::aws_smithy_types::DateTime,
    effective_from: &::aws_smithy_types::DateTime,
) -> bool {
    (signature_timestamp.secs(), signature_timestamp.subsec_nanos())
        >= (effective_from.secs(), effective_from.subsec_nanos())
}

/// Which signing profiles one profile listing enumerates: the region whose
/// endpoint is answering, the platform the caller filtered on and the statuses a
/// profile may carry to be reported.
///
/// A pagination token is issued FOR one listing and resumes only a call that
/// enumerates that same listing, so a token carried over to another region or
/// another filter set is refused rather than serving a page out of a different
/// order.
#[derive(Debug)]
pub struct ProfileListing {
    /// The region whose profiles are enumerated, and no other region's.
    pub region: String,
    /// The `platformId` filter, or `None` when the call filters on no platform.
    pub platform_id: Option<String>,
    /// The statuses a profile may carry to be reported, already resolved from
    /// the caller's `statuses` and `includeCanceled`.
    pub statuses: Vec<String>,
}

/// Version marker every pagination token carries, so a value in any other
/// shape is refused instead of being read as a cursor.
const PAGE_TOKEN_VERSION: &str = "v1";

/// What separates the listing a cursor was issued for from the element it
/// resumes at. A unit separator starts no rendered field (each one starts with a
/// digit, see [`rendered_field`]), so the boundary is unambiguous whatever a
/// request put in the fields.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// Which paginated read a cursor was issued for: the first field of every
/// canonical rendering, so a token handed out by one listing resolves to no
/// resume point in another however alike their filter fields render.
const LISTING_KIND_PROFILES: &str = "signing-profiles";

/// The platform listing's kind marker (see [`LISTING_KIND_PROFILES`]).
const LISTING_KIND_PLATFORMS: &str = "signing-platforms";

/// The signing-job listing's kind marker (see [`LISTING_KIND_PROFILES`]).
const LISTING_KIND_JOBS: &str = "signing-jobs";

/// The profile-permission listing's kind marker (see
/// [`LISTING_KIND_PROFILES`]).
const LISTING_KIND_PERMISSIONS: &str = "profile-permissions";

/// One field of a rendered listing: its byte length, then the value.
///
/// Length-prefixed rather than delimiter-joined because `platformId` and
/// `statuses` are request values this service does not constrain to an
/// alphabet, so a delimiter could be IN one of them and make the renderings of
/// two different listings collide — which would let a token issued for one
/// filter set resume another.
fn rendered_field(value: &str) -> String {
    format!("{}:{value}", value.len())
}

impl ProfileListing {
    /// What this listing enumerates, rendered canonically: two calls that
    /// enumerate the same profiles in the same order render identically however
    /// the caller spelled their filters, and two that do not never collide.
    fn canonical(&self) -> String {
        let mut statuses: Vec<&str> = self.statuses.iter().map(String::as_str).collect();
        statuses.sort_unstable();
        statuses.dedup();
        let mut canonical = String::from(PAGE_TOKEN_VERSION);
        canonical.push_str(&rendered_field(LISTING_KIND_PROFILES));
        canonical.push_str(&rendered_field(&self.region));
        canonical.push_str(&rendered_field(
            self.platform_id.as_deref().unwrap_or_default(),
        ));
        for status in statuses {
            canonical.push_str(&rendered_field(status));
        }
        canonical
    }

    /// The `nextToken` a page of this listing that stops BEFORE `resume_at`
    /// hands back.
    ///
    /// The cursor — the listing it was issued for and the profile name the next
    /// page starts at — hex-encoded and followed by its digest: opaque to the
    /// caller, which can only carry it back, and built from the unreserved
    /// characters an `@httpQuery` value travels in unescaped.
    pub fn page_token(&self, resume_at: &str) -> String {
        rendered_page_token(&self.canonical(), resume_at)
    }

    /// The profile name `token` resumes THIS listing at, or `None` when `token`
    /// is not one this listing handed out: a value whose digest does not match
    /// the cursor it carries (so anything truncated or altered in transit), or a
    /// cursor issued for another region or another filter set.
    pub fn resume_at(&self, token: &str) -> Option<String> {
        let resume_at = decoded_resume_at(&self.canonical(), token)?;
        is_profile_name(&resume_at).then_some(resume_at)
    }
}

/// Which permission statements one profile-permission listing enumerates: the
/// statements of the policy held by the profile of that name in the region whose
/// endpoint is answering.
///
/// A pagination token is issued FOR one listing and resumes only a call that
/// enumerates that same listing, so a token carried over to another region or
/// another profile is refused rather than serving a page out of a different
/// enumeration.
#[derive(Debug)]
pub struct PermissionListing {
    /// The region whose profile's policy is enumerated, and no other region's.
    pub region: String,
    /// The profile whose policy is enumerated.
    pub profile_name: String,
}

impl PermissionListing {
    /// What this listing enumerates, rendered canonically (see
    /// [`ProfileListing::canonical`]).
    fn canonical(&self) -> String {
        let mut canonical = String::from(PAGE_TOKEN_VERSION);
        canonical.push_str(&rendered_field(LISTING_KIND_PERMISSIONS));
        canonical.push_str(&rendered_field(&self.region));
        canonical.push_str(&rendered_field(&self.profile_name));
        canonical
    }

    /// The `nextToken` a page of this listing that stops BEFORE the statement
    /// `resume_at` hands back.
    pub fn page_token(&self, resume_at: &str) -> String {
        rendered_page_token(&self.canonical(), resume_at)
    }

    /// The statement id `token` resumes THIS listing at, or `None` when `token`
    /// is not one this listing handed out: a value whose digest does not match
    /// the cursor it carries, a cursor issued for another region, another profile
    /// or another paginated read, or one resuming at a value that is not a
    /// statement id at all.
    pub fn resume_at(&self, token: &str) -> Option<String> {
        let resume_at = decoded_resume_at(&self.canonical(), token)?;
        is_statement_id(&resume_at).then_some(resume_at)
    }
}

/// Which signing jobs one job listing enumerates: the region whose endpoint is
/// answering, and every filter the call narrowed its jobs with.
///
/// A pagination token is issued FOR one listing and resumes only a call that
/// enumerates that same listing, so a token carried over to another region or
/// another filter set is refused rather than serving a page out of a different
/// enumeration.
#[derive(Debug, Default)]
pub struct JobListing {
    /// The region whose jobs are enumerated, and no other region's.
    pub region: String,
    /// The `SigningStatus` filter, or `None` when the call filters on no status.
    pub status: Option<String>,
    /// The `platformId` filter, or `None` when the call filters on no platform.
    pub platform_id: Option<String>,
    /// The `requestedBy` filter — the IAM principal that requested the job — or
    /// `None` when the call filters on no principal.
    pub requested_by: Option<String>,
    /// The `jobInvoker` filter — the account that invoked the job — or `None`
    /// when the call filters on no invoker.
    pub job_invoker: Option<String>,
    /// Whether the listing is narrowed to the jobs whose signature has been
    /// revoked, from `isRevoked`.
    pub revoked_only: bool,
    /// The `signatureExpiresBefore` bound of the expiry window, or `None` when
    /// the call named no upper bound.
    pub signature_expires_before: Option<::aws_smithy_types::DateTime>,
    /// The `signatureExpiresAfter` bound of the expiry window, or `None` when the
    /// call named no lower bound.
    pub signature_expires_after: Option<::aws_smithy_types::DateTime>,
}

impl JobListing {
    /// Whether `job` is one of the jobs this listing enumerates: every filter the
    /// caller named has to equal the value the job RECORDS for that member, and a
    /// member it named no filter for narrows nothing.
    pub fn includes(&self, job: &SigningJobRecord) -> bool {
        fn kept(filter: Option<&str>, recorded: Option<&str>) -> bool {
            filter.is_none_or(|filter| recorded == Some(filter))
        }
        if !kept(self.status.as_deref(), Some(job.status.as_str())) {
            return false;
        }
        if !kept(self.platform_id.as_deref(), Some(job.platform_id.as_str())) {
            return false;
        }
        if !kept(self.requested_by.as_deref(), job.requested_by.as_deref()) {
            return false;
        }
        if !kept(self.job_invoker.as_deref(), job.job_invoker.as_deref()) {
            return false;
        }
        // `isRevoked` narrows the listing to the jobs whose signature has been
        // revoked; it is modelled with a default of `false`, which a request
        // omitting it also carries, so the unfiltered listing reports a job
        // whether or not its signature is still trusted — each one saying which
        // it is in its own `isRevoked`.
        if self.revoked_only && !job.is_revoked() {
            return false;
        }
        // The expiry window narrows the listing to the jobs whose signature
        // expires inside it, judged on the instant the job records for that
        // (`SigningJobRecord::signature_expires_at`). The two bounds partition at
        // the instant they name: a signature expiring exactly then is not before
        // it, so it is after it.
        match job.signature_expires_at() {
            Some(expires_at) => {
                self.signature_expires_before
                    .as_ref()
                    .is_none_or(|before| !at_or_after(&expires_at, before))
                    && self
                        .signature_expires_after
                        .as_ref()
                        .is_none_or(|after| at_or_after(&expires_at, after))
            }
            // A signature that is not known to expire at all falls inside no
            // window a caller can name, so only a call that bounded the expiry
            // nowhere enumerates the job.
            None => {
                self.signature_expires_before.is_none() && self.signature_expires_after.is_none()
            }
        }
    }

    /// What this listing enumerates, rendered canonically (see
    /// [`ProfileListing::canonical`]).
    fn canonical(&self) -> String {
        let mut canonical = String::from(PAGE_TOKEN_VERSION);
        canonical.push_str(&rendered_field(LISTING_KIND_JOBS));
        canonical.push_str(&rendered_field(&self.region));
        for filter in [
            &self.status,
            &self.platform_id,
            &self.requested_by,
            &self.job_invoker,
        ] {
            canonical.push_str(&rendered_field(filter.as_deref().unwrap_or_default()));
        }
        canonical.push_str(&rendered_field(if self.revoked_only {
            REVOKED_ONLY_MARKER
        } else {
            ""
        }));
        for bound in [
            &self.signature_expires_before,
            &self.signature_expires_after,
        ] {
            canonical.push_str(&rendered_field(&rendered_instant(bound.as_ref())));
        }
        canonical
    }

    /// The `nextToken` a page of this listing that stops BEFORE the job
    /// `resume_at` hands back.
    pub fn page_token(&self, resume_at: &str) -> String {
        rendered_page_token(&self.canonical(), resume_at)
    }

    /// The job id `token` resumes THIS listing at, or `None` when `token` is not
    /// one this listing handed out: a value whose digest does not match the
    /// cursor it carries, a cursor issued for another region, another filter set
    /// or another paginated read, or one resuming at a value that is not a job id
    /// at all.
    pub fn resume_at(&self, token: &str) -> Option<String> {
        let resume_at = decoded_resume_at(&self.canonical(), token)?;
        is_job_id(&resume_at).then_some(resume_at)
    }
}

/// How a listing narrowed to revoked signatures renders that filter, so it never
/// renders as the empty field an unnarrowed one does.
const REVOKED_ONLY_MARKER: &str = "revoked";

/// An instant a listing is bounded by, rendered canonically: the whole epoch
/// seconds and the sub-second part a modelled `Timestamp` carries, so two calls
/// that named the same instant render identically however the request spelled it,
/// and the empty string for a call that named no bound.
fn rendered_instant(instant: Option<&::aws_smithy_types::DateTime>) -> String {
    instant
        .map(|instant| format!("{}.{:09}", instant.secs(), instant.subsec_nanos()))
        .unwrap_or_default()
}

/// Which signing platforms one platform listing enumerates: the catalog this
/// service publishes, narrowed to the category, partner and target the caller
/// filtered on.
///
/// A pagination token is issued FOR one listing and resumes only a call that
/// enumerates that same listing, so a token carried over to another filter set
/// is refused rather than serving a page out of a different enumeration.
#[derive(Debug, Default)]
pub struct PlatformListing {
    /// The `category` filter, or `None` when the call filters on no category.
    pub category: Option<String>,
    /// The `partner` filter, or `None` when the call filters on no partner.
    pub partner: Option<String>,
    /// The `target` filter, or `None` when the call filters on no target.
    pub target: Option<String>,
}

impl PlatformListing {
    /// Whether `entry` is one of the platforms this listing enumerates: each
    /// filter the caller named has to equal the value the catalog publishes for
    /// that member of the platform, and a member it named no filter for narrows
    /// nothing.
    pub fn includes(&self, entry: &SigningPlatformEntry) -> bool {
        fn kept(filter: Option<&str>, published: &str) -> bool {
            filter.is_none_or(|filter| filter == published)
        }
        kept(self.category.as_deref(), entry.category)
            && kept(self.partner.as_deref(), entry.partner)
            && kept(self.target.as_deref(), entry.target())
    }

    /// The platforms this listing enumerates, in the order the catalog publishes
    /// them — the one order every page of this listing is cut out of.
    pub fn platforms(&self) -> impl Iterator<Item = &'static SigningPlatformEntry> + '_ {
        SIGNING_PLATFORM_CATALOG
            .iter()
            .filter(|entry| self.includes(entry))
    }

    /// What this listing enumerates, rendered canonically (see
    /// [`ProfileListing::canonical`]).
    fn canonical(&self) -> String {
        let mut canonical = String::from(PAGE_TOKEN_VERSION);
        canonical.push_str(&rendered_field(LISTING_KIND_PLATFORMS));
        for filter in [&self.category, &self.partner, &self.target] {
            canonical.push_str(&rendered_field(filter.as_deref().unwrap_or_default()));
        }
        canonical
    }

    /// The `nextToken` a page of this listing that stops BEFORE the platform
    /// `resume_at` hands back.
    pub fn page_token(&self, resume_at: &str) -> String {
        rendered_page_token(&self.canonical(), resume_at)
    }

    /// The platform id `token` resumes THIS listing at, or `None` when `token`
    /// is not one this listing handed out: a value whose digest does not match
    /// the cursor it carries, a cursor issued for another filter set or another
    /// paginated read, or one resuming at a platform this service publishes no
    /// entry for.
    pub fn resume_at(&self, token: &str) -> Option<String> {
        let resume_at = decoded_resume_at(&self.canonical(), token)?;
        signing_platform(&resume_at).map(|entry| entry.platform_id.to_string())
    }
}

/// The `nextToken` a page of the listing rendered as `canonical` hands back when
/// the next page starts at `resume_at`.
///
/// The cursor — the listing it was issued for and the element the next page
/// starts at — hex-encoded and followed by its digest: opaque to the caller,
/// which can only carry it back, and built from the unreserved characters an
/// `@httpQuery` value travels in unescaped.
fn rendered_page_token(canonical: &str, resume_at: &str) -> String {
    let cursor = format!("{canonical}{PAGE_TOKEN_SEPARATOR}{resume_at}");
    format!("{}{}", ::hex::encode(&cursor), cursor_digest(&cursor))
}

/// The resume point `token` carries for the listing rendered as `canonical`, or
/// `None` when it is not a token that listing handed out: a value whose digest
/// does not match the cursor it carries (so anything truncated or altered in
/// transit), or a cursor issued for a different listing.
///
/// The resume point is returned as the cursor spelled it; it is for the CALLER
/// to hold it to the form its own elements take.
fn decoded_resume_at(canonical: &str, token: &str) -> Option<String> {
    // Every token is hex digits, so the split below lands on a character
    // boundary and a value in any other alphabet is already not one of ours.
    if !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let (encoded, digest) = token.split_at(token.len().checked_sub(DIGEST_HEX_LEN)?);
    let cursor = String::from_utf8(::hex::decode(encoded).ok()?).ok()?;
    if !cursor_digest(&cursor).eq_ignore_ascii_case(digest) {
        return None;
    }
    let issued_for = format!("{canonical}{PAGE_TOKEN_SEPARATOR}");
    cursor
        .strip_prefix(&issued_for)
        .map(|resume_at| resume_at.to_string())
}

/// How many hex digits of a cursor's digest a pagination token carries.
const DIGEST_HEX_LEN: usize = 16;

/// A cursor's digest, as the hex digits a token ends with.
///
/// This is a CHECKSUM, not a signature: it makes a token tamper-EVIDENT — a
/// value truncated or altered carries no digest of the cursor it decodes to, so
/// it resolves to no resume point at all instead of to one the service never
/// handed out — and it does not make a token unforgeable, since nothing here is
/// keyed. That is what this pagination needs: what a resumed page must never do
/// is serve a position out of a DIFFERENT enumeration, and a cursor a client
/// reconstructed for this same listing still resolves to a name within it.
fn cursor_digest(cursor: &str) -> String {
    ::hex::encode(<::sha2::Sha256 as ::sha2::Digest>::digest(cursor.as_bytes()))
        [..DIGEST_HEX_LEN]
        .to_string()
}

/// The region this request was signed for — the region whose endpoint is being
/// addressed, which is the region its resources live in and the one a profile ARN
/// names.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The IAM user this emulator's credentials speak for.
///
/// This emulator publishes ONE long-term credential, and it belongs to this IAM
/// user: `sts:GetCallerIdentity` answers
/// `arn:aws:iam::<account>:user/emulator` for every request signed with it, so
/// that is the identity the emulator resolves for a caller and the identity
/// every service here reports for one.
const CALLER_USER_NAME: &str = "emulator";

/// The IAM principal the request's credentials resolve to — the identity AWS
/// reports as the caller of an operation (a job's `requestedBy`, a revocation's
/// `revokedBy`).
///
/// An access key id is the NAME OF A CREDENTIAL, not of a principal, so it is
/// not what such a member carries: the principal is the IAM identity the
/// credential belongs to, which for every credential this emulator accepts is
/// the one user it publishes ([`CALLER_USER_NAME`]) — the same identity STS
/// answers a caller-identity question with, and therefore stable across calls
/// and across runs. `None` for an unsigned request, which presents no credential
/// to resolve and so names no principal at all.
pub fn caller_principal_arn(parts: &::http::request::Parts) -> Option<String> {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|_| ::engine::emulator_arn("iam", "", &format!("user/{CALLER_USER_NAME}")))
}

/// The AWS ACCOUNT the request's credentials belong to — the account AWS reports
/// as having invoked an operation (a signing job's `jobInvoker`).
///
/// This emulator presents exactly ONE account, and every credential it accepts
/// belongs to that account, so a signed request resolves to it. `None` for an
/// unsigned request, which presents no credential and so names no account at
/// all — the same split [`caller_principal_arn`] makes, because the two answer
/// the same question about the same credential.
pub fn caller_account_id(parts: &::http::request::Parts) -> Option<String> {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|_| ::engine::emulator_account_id().to_string())
}

/// `time` as the wire timestamp.
pub fn to_timestamp(time: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(time)
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSignerWallabyserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSignerWallabyserviceState>)
}
