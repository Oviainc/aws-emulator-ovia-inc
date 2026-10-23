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
//!   platform, version and signing material it SNAPSHOTS, and reported by
//!   `DescribeSigningJob`.
//!
//! Every handler reads and writes the one record below through [`service_state`];
//! nothing in this service answers from a literal.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Destination, S3SignedObject, SignatureValidityPeriod, SignedObject, SigningMaterial,
    SigningPlatformOverrides, SigningProfile, SigningProfileRevocationRecord, Source,
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

/// One signing profile, as this emulator persists it.
///
/// `arn` and `profile_version_arn` are stored rather than recomputed per read, so
/// a profile created by a request signed for one region keeps reporting that
/// region's ARN.
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

    /// The stored signing parameters, as the wire map — absent when there are none.
    pub fn signing_parameter_map(&self) -> Option<HashMap<String, String>> {
        non_empty_map(&self.signing_parameters)
    }
}

/// Per-service state for com-amazonaws-signer-wallabyservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsSignerWallabyserviceState {
    /// Every signing profile, keyed by `profileName` — the name every operation
    /// and every profile ARN resolves through. A `BTreeMap` is what gives
    /// `ListSigningProfiles` the stable total order its pagination token indexes
    /// into.
    pub profiles: BTreeMap<String, SigningProfileRecord>,
    /// Every signing job, keyed by `jobId`; ids sort in the order they were
    /// minted.
    pub jobs: BTreeMap<String, SigningJobRecord>,
    /// A `StartSigningJob` `clientRequestToken` to the job that token started, so
    /// a retried start answers with the first job instead of a second one.
    pub job_tokens: HashMap<String, String>,
    /// The counter profile versions are minted from.
    pub profile_version_seq: u64,
    /// The counter profile tokens are minted from.
    pub profile_token_seq: u64,
    /// The counter job ids are minted from.
    pub job_id_seq: u64,
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

    /// The job `job_id` names, with any completion the clock owes it already
    /// materialised.
    pub fn settled_job_mut(&mut self, job_id: &str, now: SystemTime) -> Option<&mut SigningJobRecord> {
        let job = self.jobs.get_mut(job_id)?;
        job.settle(now);
        Some(job)
    }

    /// The profile a tagging call's `resourceArn` names.
    ///
    /// Resolution is by the ARN the profile was STORED with, so a label naming
    /// another account's or another region's signing profile resolves to nothing
    /// rather than to this account's profile of the same name.
    pub fn profile_for_tagging(
        &self,
        resource_arn: &str,
    ) -> Result<&SigningProfileRecord, TaggingArnProblem> {
        let name = profile_name_in_arn(resource_arn).ok_or(TaggingArnProblem::Malformed)?;
        self.profiles
            .get(name)
            .filter(|profile| profile.arn == resource_arn)
            .ok_or(TaggingArnProblem::NoSuchProfile)
    }

    /// [`Self::profile_for_tagging`], for a call that CHANGES the profile's tags.
    pub fn profile_for_tagging_mut(
        &mut self,
        resource_arn: &str,
    ) -> Result<&mut SigningProfileRecord, TaggingArnProblem> {
        let name = profile_name_in_arn(resource_arn).ok_or(TaggingArnProblem::Malformed)?;
        self.profiles
            .get_mut(name)
            .filter(|profile| profile.arn == resource_arn)
            .ok_or(TaggingArnProblem::NoSuchProfile)
    }
}

/// The id namespace signing-job ids are minted in.
const JOB_ID_NAMESPACE: u64 = 0;

/// The id namespace profile tokens are minted in, so no token ever collides with
/// a job id.
const PROFILE_TOKEN_NAMESPACE: u64 = 1;

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

/// The profile name inside `arn`, when `arn` is a well-formed AWS Signer
/// signing-profile ARN — `arn:<partition>:signer:<region>:<account>:/signing-profiles/<name>`,
/// the form [`profile_arn`] builds.
///
/// `None` for anything else: an ARN of another service, one whose resource is not
/// a signing profile, one carrying a profile VERSION after the name (tags belong
/// to the profile, not to one of its versions), or a name outside
/// [`is_profile_name`]'s alphabet. It is a judgement on the STRING alone, so it
/// says nothing about whether such a profile exists.
pub fn profile_name_in_arn(arn: &str) -> Option<&str> {
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
    if fields.next()?.is_empty() || fields.next()?.is_empty() {
        return None;
    }
    let name = fields.next()?.strip_prefix(PROFILE_ARN_RESOURCE_PREFIX)?;
    is_profile_name(name).then_some(name)
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

/// The first key of `keys` that carries [`RESERVED_TAG_KEY_PREFIX`], or `None`
/// when none does.
pub fn reserved_tag_key<'a, I>(keys: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter()
        .find(|key| key.starts_with(RESERVED_TAG_KEY_PREFIX))
}

/// The refusal prose for a tag key inside AWS's reserved namespace.
pub fn reserved_tag_key_message(key: &str) -> String {
    format!("tag key {key} is in the reserved prefix {RESERVED_TAG_KEY_PREFIX}")
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

/// One signing platform this emulator publishes.
#[derive(Debug, Clone, Copy)]
pub struct SigningPlatformEntry {
    /// The `platformId` a `PutSigningProfile` names.
    pub platform_id: &'static str,
    /// The `displayName` AWS Signer publishes for it.
    pub display_name: &'static str,
    /// Whether a profile on this platform must name the ACM certificate its
    /// signatures are issued under (`signingMaterial`).
    pub requires_signing_material: bool,
}

/// The signing platforms this emulator publishes, with the two facts the
/// profile/job slice needs about each.
///
/// Source: the `platformId` / `displayName` pairs of the code-signing platforms
/// AWS Signer documents (AWS IoT Device Management, AWS Lambda, FreeRTOS). The
/// catalog itself is what `ListSigningPlatforms` and `GetSigningPlatform` serve —
/// both are still stubs, with their own gap issues — so the two lookups below are
/// the ones this slice needs.
///
/// `requires_signing_material` is a SEPARATE fact from publishing a display name,
/// stated per entry rather than inferred from membership: for each of these
/// platforms the recorded `PutSigningProfile` accepts a profile created with no
/// `signingMaterial`, while a platform this catalog does not name is refused
/// ("signing material cannot be null."). A platform added here for its display
/// name alone must therefore state its own answer — see
/// [`platform_requires_signing_material`] for what an UNPUBLISHED platform
/// answers and why.
pub const SIGNING_PLATFORM_CATALOG: &[SigningPlatformEntry] = &[
    SigningPlatformEntry {
        platform_id: "AWSIoTDeviceManagement-SHA256-ECDSA",
        display_name: "AWS IoT Device Management SHA256-ECDSA",
        requires_signing_material: false,
    },
    SigningPlatformEntry {
        platform_id: "AWSLambda-SHA384-ECDSA",
        display_name: "AWS Lambda",
        requires_signing_material: false,
    },
    SigningPlatformEntry {
        platform_id: "AmazonFreeRTOS-Default",
        display_name: "AmazonFreeRTOS SHA256-ECDSA",
        requires_signing_material: false,
    },
    SigningPlatformEntry {
        platform_id: "AmazonFreeRTOS-TI-CC3220SF",
        display_name: "AmazonFreeRTOS SHA1-RSA CC3220SF-Format",
        requires_signing_material: false,
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
/// platform id [`SIGNING_PLATFORM_CATALOG`] holds no entry for — such a platform
/// simply has no display name to report.
pub fn platform_display_name(platform_id: &str) -> Option<String> {
    signing_platform(platform_id).map(|entry| entry.display_name.to_string())
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

/// True when `value` satisfies `ProfileName`'s `@length(2, 64)` and
/// `^[a-zA-Z0-9_]{2,}$`.
///
/// The boundary validator already refuses a modelled `ProfileName` member, so
/// this is for a value the model does not constrain: a `nextToken`, which this
/// service's profile pagination carries a profile name in.
pub fn is_profile_name(value: &str) -> bool {
    (2..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}

/// The region this request was signed for — what a profile ARN names.
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
