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
    SignatureValidityPeriod, SignedObject, SigningConfiguration, SigningImageFormat,
    SigningJobRevocationRecord, SigningMaterial, SigningPlatformOverrides, SigningProfile,
    SigningProfileRevocationRecord, Source,
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

    /// Whether this profile's tags may still be CHANGED.
    ///
    /// `BadRequestException` documents the one status that forbids it: the shape
    /// "also occurs when you call a tagging API on a cancelled signing profile"
    /// (`smithy_query(query="shape", shape="BadRequestException")`). Nothing
    /// documents a refusal for any other status, and a canceled profile is the
    /// one Signer schedules for deletion, so `Active` and `Revoked` profiles
    /// accept tag writes.
    pub fn accepts_tag_writes(&self) -> bool {
        self.status != PROFILE_STATUS_CANCELED
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
    pub fn revoke(
        &mut self,
        reason: &str,
        effective_time: ::aws_smithy_types::DateTime,
        revoked_by: Option<String>,
        now: SystemTime,
    ) {
        self.status = PROFILE_STATUS_REVOKED.to_string();
        self.status_reason = Some(reason.to_string());
        self.revocation_record = Some(SigningProfileRevocationRecord {
            revocation_effective_from: Some(effective_time),
            revoked_at: Some(to_timestamp(now)),
            revoked_by,
        });
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

    /// The stored signing parameters, as the wire map — absent when there are none.
    pub fn signing_parameter_map(&self) -> Option<HashMap<String, String>> {
        non_empty_map(&self.signing_parameters)
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
    pub fn insert_profile(&mut self, region: &str, record: SigningProfileRecord) {
        let key = profile_key(region, &record.profile_name);
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
    // Region and account: present, and non-empty for a profile ARN (Signer is a
    // regional service and a profile belongs to one account).
    let region = fields.next()?;
    if region.is_empty() || fields.next()?.is_empty() {
        return None;
    }
    let name = fields.next()?.strip_prefix(PROFILE_ARN_RESOURCE_PREFIX)?;
    is_profile_name(name).then_some((region, name))
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
    pub fn message(self, resource_arn: &str) -> String {
        match self {
            Self::Malformed => {
                format!("{resource_arn} is not a valid signing profile resource ARN")
            }
            Self::NoSuchProfile => {
                format!("profile with arn {resource_arn} does not exists")
            }
        }
    }
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

/// The refusal prose for a tag write on a profile whose status forbids it.
pub fn tag_write_refused_message(profile_name: &str, status: &str) -> String {
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
    pub requires_signing_material: bool,
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
/// stated per entry rather than inferred from membership: for the four
/// code-signing platforms the recorded `PutSigningProfile` accepts a profile
/// created with no `signingMaterial`, while the container-image platform and any
/// platform outside this catalog are refused ("signing material cannot be
/// null."). See [`platform_requires_signing_material`] for what an UNPUBLISHED
/// platform answers and why.
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
        requires_signing_material: false,
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
        requires_signing_material: false,
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
        requires_signing_material: false,
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
        requires_signing_material: true,
    },
];

/// The catalog entry for `platform_id`, or `None` for a platform this emulator
/// publishes no entry for.
pub fn signing_platform(platform_id: &str) -> Option<&'static SigningPlatformEntry> {
    SIGNING_PLATFORM_CATALOG
        .iter()
        .find(|entry| entry.platform_id == platform_id)
}

/// The display name AWS Signer publishes for a signing platform, or `None` for a
/// platform [`SIGNING_PLATFORM_CATALOG`] publishes no display name for (an id it
/// holds no entry for at all, or one whose entry publishes none) — such a
/// platform simply has no display name to report.
pub fn platform_display_name(platform_id: &str) -> Option<String> {
    signing_platform(platform_id)
        .and_then(|entry| entry.display_name)
        .map(str::to_string)
}

/// The refusal prose for a `platformId` this emulator publishes no platform for.
pub fn platform_not_found_message(platform_id: &str) -> String {
    format!("Platform with id {platform_id} does not exist")
}

/// Whether a profile on `platform_id` must carry `signingMaterial` — the ACM
/// certificate its signatures are issued under.
///
/// A published platform answers with its own
/// [`SigningPlatformEntry::requires_signing_material`]. A platform this emulator
/// publishes NO entry for defaults to requiring it: a profile on a platform whose
/// signing configuration is unknown here holds the certificate itself, which is
/// what the recorded `PutSigningProfile` shows — it refuses such a request with
/// "signing material cannot be null." rather than accepting it.
///
/// The model cannot express the distinction: `signingMaterial` is one optional
/// member for every platform
/// (`smithy_query(query="shape",shape="PutSigningProfileRequest")`).
pub fn platform_requires_signing_material(platform_id: &str) -> bool {
    signing_platform(platform_id).is_none_or(|entry| entry.requires_signing_material)
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

/// Version marker every profile pagination token carries, so a value in any
/// other shape is refused instead of being read as a cursor.
const PROFILE_PAGE_TOKEN_VERSION: &str = "v1";

/// What separates the listing a cursor was issued for from the profile name it
/// resumes at. A unit separator starts no rendered field (each one starts with a
/// digit, see [`ProfileListing::canonical`]), so the boundary is unambiguous
/// whatever a request put in the fields.
const PROFILE_PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

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
        let mut canonical = String::from(PROFILE_PAGE_TOKEN_VERSION);
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
        let cursor = format!(
            "{}{PROFILE_PAGE_TOKEN_SEPARATOR}{resume_at}",
            self.canonical()
        );
        format!("{}{}", ::hex::encode(&cursor), cursor_digest(&cursor))
    }

    /// The profile name `token` resumes THIS listing at, or `None` when `token`
    /// is not one this listing handed out: a value whose digest does not match
    /// the cursor it carries (so anything truncated or altered in transit), or a
    /// cursor issued for another region or another filter set.
    pub fn resume_at(&self, token: &str) -> Option<String> {
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
        let issued_for = format!("{}{PROFILE_PAGE_TOKEN_SEPARATOR}", self.canonical());
        let resume_at = cursor.strip_prefix(&issued_for)?;
        is_profile_name(resume_at).then(|| resume_at.to_string())
    }
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

/// The caller a signing job records as having requested it.
///
/// AWS reports the IAM principal, which needs an IAM directory this emulator
/// does not model; the SigV4 access key id the request was signed with is the
/// caller identity the request itself carried, and it is `None` for an unsigned
/// one.
pub fn request_principal(parts: &::http::request::Parts) -> Option<String> {
    ::emulator_server::sigv4_credential_scope(&parts.headers).map(|scope| scope.access_key_id)
}

/// The caller as an IAM principal ARN — the form AWS reports a revoker in.
///
/// The identity is still the one [`request_principal`] reads off the request (no
/// IAM directory exists to resolve a friendly name against), rendered as the
/// `arn:aws:iam::<account>:user/<identity>` an AWS caller is named by, so a
/// client that parses the revoker as an ARN still works. `None` for an unsigned
/// request, which carries no identity at all.
pub fn request_principal_arn(parts: &::http::request::Parts) -> Option<String> {
    request_principal(parts)
        .map(|identity| ::engine::emulator_arn("iam", "", &format!("user/{identity}")))
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
