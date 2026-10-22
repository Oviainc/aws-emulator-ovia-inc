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

/// `SigningStatus`'s values, spelled as the model spells them (see
/// [`crate::ops::types::SIGNING_STATUS_VALUES`]).
pub const JOB_STATUS_IN_PROGRESS: &str = "InProgress";
/// The status a signing job reports once the clock has made its completion due.
pub const JOB_STATUS_SUCCEEDED: &str = "Succeeded";

/// The largest page `ListSigningProfiles` serves, and the page size it uses when
/// the request names no `maxResults`: `ListSigningProfilesRequest$maxResults` is
/// modelled `@range(min: 1, max: 25)`.
pub const PROFILE_PAGE_LIMIT: i32 = 25;

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

/// The display name AWS Signer publishes for a signing platform, or `None` for a
/// platform id this emulator holds no entry for.
///
/// Source: the `platformId` / `displayName` pairs of the signing platforms AWS
/// Signer documents. The catalog itself is what `ListSigningPlatforms` and
/// `GetSigningPlatform` serve — both are still stubs, with their own gap issues —
/// so this is the one lookup into it the profile/job slice needs, and a platform
/// outside it simply has no display name to report.
pub fn platform_display_name(platform_id: &str) -> Option<String> {
    const CATALOG: &[(&str, &str)] = &[
        (
            "AWSIoTDeviceManagement-SHA256-ECDSA",
            "AWS IoT Device Management SHA256-ECDSA",
        ),
        ("AWSLambda-SHA384-ECDSA", "AWS Lambda"),
        ("AmazonFreeRTOS-Default", "AmazonFreeRTOS SHA256-ECDSA"),
        (
            "AmazonFreeRTOS-TI-CC3220SF",
            "AmazonFreeRTOS SHA1-RSA CC3220SF-Format",
        ),
    ];
    CATALOG
        .iter()
        .find(|(id, _)| *id == platform_id)
        .map(|(_, display)| (*display).to_string())
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
