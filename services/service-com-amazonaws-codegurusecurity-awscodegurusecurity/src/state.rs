// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codegurusecurity-awscodegurusecurity service.
//!
//! ONE store behind [`service_state`], shared by every handler of this service: the
//! upload artifacts `CreateUploadUrl` mints (each with the code resource uploaded to its
//! target), the scans `CreateScan` records (each with its runs), the findings both
//! `GetFindings` and `BatchGetFindings` read, and the idempotency ledger that makes a
//! `CreateScan` retry answer with its original run.
//!
//! # Every caller-driven collection here is BOUNDED
//!
//! Each store a request can grow carries an explicit maximum, enforced in the handler
//! that writes it, so no sequence of requests can grow the emulator's heap without end:
//!
//! * [`UPLOAD_ARTIFACT_QUOTA`] caps `upload_artifacts`, and an artifact also expires
//!   [`UPLOAD_ARTIFACT_TTL_MILLIS`] after it was minted ([`Store::expire_due_artifacts`]).
//! * an upload to a minted target is ACCEPTED whatever the size of a code archive, and
//!   the bound sits on the RETAINED side instead: [`Store::receive_upload`], the only
//!   writer of uploaded bytes, records the archive's length and a digest of all of its
//!   bytes and keeps at most [`UPLOAD_RETAINED_PREFIX_MAX_BYTES`] of its content, so what
//!   every target holds together cannot exceed that times [`UPLOAD_ARTIFACT_QUOTA`]. One
//!   upload is bounded too, by what this process can hold at once
//!   ([`UPLOAD_ACCEPTED_MAX_BYTES`], read off the declared length before the body is
//!   touched), which is far above any archive a scan is run on.
//! * [`SCAN_QUOTA`] caps `scans`, and [`RUNS_PER_SCAN_QUOTA`] caps the runs of one scan,
//!   so the (scans x runs) product is bounded.
//! * [`TAGS_PER_SCAN_QUOTA`] caps the tags of one scan, enforced in every writer of the
//!   tag store (`CreateScan` and `TagResource`, through [`tag_count_after_merge`] and
//!   [`tag_quota_refusal`]), so repeatedly tagging one scan cannot grow its tag store
//!   without end.
//! * `idempotency` gains one entry only where a run was created, so it is bounded by the
//!   same (scans x runs) product, and an entry is dropped once its
//!   [`CLIENT_TOKEN_WINDOW_MILLIS`] has passed ([`Store::expire_due_client_tokens`]).
//! * `findings` is written only by a scan run's analysis, so no request path grows it;
//!   the readers' page sizes are clamped to the modelled `maxResults` maxima.
//!
//! Per-request CPU is bounded too: the clock-owed work the readers do is driven by the
//! `pending_runs` / `artifact_expiry` / `idempotency_expiry` due-queues, so each call
//! touches only the entries that actually came due rather than walking everything earlier
//! requests stored.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ops::types::{
    Finding, FindingMetricsValuePerSeverity, ValidationException, ValidationExceptionField,
};

/// The region an emulated request is for: the region of its SigV4 credential scope,
/// or the emulator's documented default when the request carries no scope (see
/// `emulator_server::emulator_arn`, "emulators default to `us-east-1`").
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ScanState`, from `types::shared_types::SCAN_STATE_VALUES`
/// (`Failed | InProgress | Successful`).
pub const SCAN_STATE_IN_PROGRESS: &str = "InProgress";
/// `ScanState`; see [`SCAN_STATE_IN_PROGRESS`]. The terminal state of a run that was
/// carried out: "Returns either InProgress, Successful, or Failed"
/// (`GetScanResponse$scanState`).
pub const SCAN_STATE_SUCCESSFUL: &str = "Successful";
/// `ScanState`; see [`SCAN_STATE_IN_PROGRESS`]. The terminal state of a run that could
/// not be carried out, and the only state an `errorMessage` goes with ("Details about the
/// error that causes a scan to fail", `GetScanResponse$errorMessage`).
pub const SCAN_STATE_FAILED: &str = "Failed";

/// `ScanType`, from `types::shared_types::SCAN_TYPE_VALUES` (`Express | Standard`).
/// The model documents `scanType` as "Defaults to Standard type if missing".
pub const SCAN_TYPE_STANDARD: &str = "Standard";
/// `ScanType`; see [`SCAN_TYPE_STANDARD`].
pub const SCAN_TYPE_EXPRESS: &str = "Express";

/// `AnalysisType`, from `types::shared_types::ANALYSIS_TYPE_VALUES` (`All | Security`).
/// The model documents `analysisType` as "Defaults to Security type if missing".
pub const ANALYSIS_TYPE_SECURITY: &str = "Security";

/// `Status`, from `types::shared_types::STATUS_VALUES` (`All | Closed | Open`).
pub const STATUS_ALL: &str = "All";
/// `Status`; see [`STATUS_ALL`]. The status an OPEN finding carries, which is what the
/// metrics operations count.
pub const STATUS_OPEN: &str = "Open";
/// `Status`; see [`STATUS_ALL`]. The status a finding carries once it is no longer
/// open, which `ListFindingsMetrics` counts as a close.
pub const STATUS_CLOSED: &str = "Closed";

/// `Severity`, from `types::shared_types::SEVERITY_VALUES`
/// (`Critical | High | Info | Low | Medium`). One constant per value, because the
/// metrics operations report one count per severity
/// (`FindingMetricsValuePerSeverity`).
pub const SEVERITY_INFO: &str = "Info";
/// `Severity`; see [`SEVERITY_INFO`].
pub const SEVERITY_LOW: &str = "Low";
/// `Severity`; see [`SEVERITY_INFO`].
pub const SEVERITY_MEDIUM: &str = "Medium";
/// `Severity`; see [`SEVERITY_INFO`].
pub const SEVERITY_HIGH: &str = "High";
/// `Severity`; see [`SEVERITY_INFO`].
pub const SEVERITY_CRITICAL: &str = "Critical";

/// Milliseconds in a day, on the scale [`now_millis`] reads. Unix time counts no leap
/// seconds, so every day on this scale is exactly this long — which is what lets
/// [`start_of_utc_day_millis`] floor an instant to its day arithmetically.
pub const MILLIS_PER_DAY: i64 = 24 * 60 * 60 * 1_000;

/// `ErrorCode`, from `types::shared_types::ERROR_CODE_VALUES`. `BatchGetFindings`
/// classifies each identifier it cannot resolve with one of these.
pub const ERROR_CODE_INVALID_SCAN_NAME: &str = "INVALID_SCAN_NAME";
/// `ErrorCode`; see [`ERROR_CODE_INVALID_SCAN_NAME`]. The classification for an
/// identifier that is well formed and names nothing this account holds.
pub const ERROR_CODE_ITEM_DOES_NOT_EXIST: &str = "ITEM_DOES_NOT_EXIST";
/// `ErrorCode`; see [`ERROR_CODE_INVALID_SCAN_NAME`].
pub const ERROR_CODE_DUPLICATE_IDENTIFIER: &str = "DUPLICATE_IDENTIFIER";
/// `ErrorCode`; see [`ERROR_CODE_INVALID_SCAN_NAME`]. The classification for an
/// identifier whose `findingId` is not a value this service hands out at all
/// ([`is_minted_identifier`]).
pub const ERROR_CODE_INVALID_FINDING_ID: &str = "INVALID_FINDING_ID";

/// The signing name this service answers under, used for its ARNs and for the
/// `serviceCode` of a quota refusal.
pub const SERVICE_CODE: &str = "codeguru-security";

/// The most code artifacts this emulator keeps minted at once. A quota, not a fidelity
/// claim: it is what stops `CreateUploadUrl` growing the store without end. Reached, the
/// call is refused with the modelled `ThrottlingException` (HTTP 429) carrying
/// [`QUOTA_CODE_UPLOAD_ARTIFACTS`].
pub const UPLOAD_ARTIFACT_QUOTA: usize = 1_000;

/// How long a minted upload target stays usable. CHOSEN, not documented: AWS publishes
/// no lifetime for `CreateUploadUrl`'s pre-signed URL, only that it IS pre-signed, and a
/// pre-signed S3 URL is always time-limited. Expiry is driven from the recorded
/// `created_at_millis`, so an artifact nobody scanned is released rather than kept.
pub const UPLOAD_ARTIFACT_TTL_MILLIS: i64 = 15 * 60 * 1_000;

/// The most scans this emulator keeps at once; see [`UPLOAD_ARTIFACT_QUOTA`].
pub const SCAN_QUOTA: usize = 1_000;

/// The most tags one scan holds, so repeated `TagResource` calls cannot grow a scan's
/// tag store without end. A STORE BOUND chosen by this emulator, not a published AWS
/// quota: it is the modelled maximum of `TagMap` itself
/// (`smithy_query(query="shape",shape="TagMap")`: length 0..200), which is what ONE
/// request may carry, applied to the scan's accumulated tags as well — AWS publishes no
/// tags-per-scan limit for CodeGuru Security, so nothing here should be read as its
/// recorded behaviour.
pub const TAGS_PER_SCAN_QUOTA: usize = 200;

/// The tag-key prefix AWS keeps for the tags it applies itself. AWS's tagging rules
/// state that the `aws:` prefix is reserved for AWS use and that a caller can neither
/// create nor edit nor delete a tag whose key carries it, so the prefix is not part of
/// the namespace a tagging request may name. `TagKey` carries only a `@length`
/// (`smithy_query(query="shape",shape="TagKey")`: 1..128, no `@pattern`), so this is a
/// semantic rule the boundary validator cannot express and each writer of the tag store
/// enforces through [`is_reserved_tag_key`].
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Whether `key` lies in the namespace [`RESERVED_TAG_KEY_PREFIX`] reserves.
///
/// Compared WITHOUT regard to case, so a differently cased spelling of the prefix does
/// not slip a system-looking tag past the rule. `get(..)` rather than slicing, so a key
/// whose prefix-length byte boundary falls inside a multi-byte character is simply not
/// reserved instead of panicking.
pub fn is_reserved_tag_key(key: &str) -> bool {
    key.get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
}

/// The first key of `keys` that [`is_reserved_tag_key`], in the order given, or `None`
/// when the request names no reserved key. The one place the tag writers ask the
/// question, so `CreateScan`, `TagResource` and `UntagResource` all judge a key the
/// same way.
pub fn first_reserved_tag_key<'a, I>(keys: I) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    keys.into_iter().find(|key| is_reserved_tag_key(key))
}

/// The modelled refusal of a request naming the reserved tag key `key` in member
/// `member`: a `ValidationException` (HTTP 400), whose message names the member, the
/// offending key and the constraint it breaks.
pub fn reserved_tag_key_refusal(member: &str, key: &str) -> ValidationException {
    ValidationException {
        error_code: "ValidationException".to_string(),
        message: format!(
            "{member} names the tag key {key}, which begins with the reserved \
             {RESERVED_TAG_KEY_PREFIX} prefix: that namespace holds the tags Amazon Web \
             Services applies itself, and a caller can neither create, edit nor delete one"
        ),
        reason: "fieldValidationFailed".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "a tag key must not begin with {RESERVED_TAG_KEY_PREFIX}; {key} does"
            ),
        }]),
    }
}

/// Apply the tag map `requested` to the tags `held` on a resource the way a tagging
/// input is applied: every key `requested` names takes the value it carries, and every
/// key it does not name is left exactly as it is.
///
/// Tagging an AWS resource is ADDITIVE — the only operation that removes a tag key is
/// `UntagResource` — so every writer of a scan's tag store goes through here rather than
/// assigning the request's map over the stored one, which would delete the keys the
/// request happens not to mention.
pub fn merge_tags(held: &mut BTreeMap<String, String>, requested: &HashMap<String, String>) {
    for (key, value) in requested {
        held.insert(key.clone(), value.clone());
    }
}

/// How many tags a resource would hold once [`merge_tags`] applied `requested` to
/// `held`: the keys it already carries plus only those `requested` keys that are new,
/// since a key already held takes a new value rather than adding an entry.
pub fn tag_count_after_merge(
    held: &BTreeMap<String, String>,
    requested: &HashMap<String, String>,
) -> usize {
    let added = requested
        .keys()
        .filter(|key| !held.contains_key(key.as_str()))
        .count();
    held.len().saturating_add(added)
}

/// The modelled refusal of a request whose tag map would leave `would` tags on the scan
/// `resource`, past [`TAGS_PER_SCAN_QUOTA`]: a `ValidationException` (HTTP 400) naming
/// the member at fault, the bound and how much room is left. Shared by every writer of
/// the tag store, so the bound is reported the same way whichever call reaches it.
pub fn tag_quota_refusal(
    member: &str,
    resource: &str,
    held: usize,
    would: usize,
) -> ValidationException {
    ValidationException {
        error_code: "ValidationException".to_string(),
        message: format!(
            "a scan holds at most {TAGS_PER_SCAN_QUOTA} tags; this request would leave \
             {would} on {resource}"
        ),
        reason: "fieldValidationFailed".to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "remove tags from the scan, or send at most {remaining} new keys",
                remaining = TAGS_PER_SCAN_QUOTA.saturating_sub(held),
            ),
        }]),
    }
}

/// The most runs (revisions) one scan keeps; see [`UPLOAD_ARTIFACT_QUOTA`]. Together
/// with [`SCAN_QUOTA`] this bounds the total number of runs, and with it the size of the
/// idempotency ledger and the work the due-queues can hold.
pub const RUNS_PER_SCAN_QUOTA: usize = 100;

/// How long a `CreateScan` client token stands for the request it was first seen on.
///
/// CHOSEN, not documented: AWS publishes no window for `CreateScanRequest$clientToken`,
/// only that the value exists "to prevent the accidental creation of duplicate scans if
/// there are failures and retries" — a retry horizon, not a permanent reservation. A
/// bounded window is what makes the token behave as a retry marker rather than as a
/// lifetime lock: once it has passed, the token names no request and the next call
/// carrying it is a fresh one, which is also what keeps the ledger from holding a record
/// for the lifetime of the process.
pub const CLIENT_TOKEN_WINDOW_MILLIS: i64 = 24 * 60 * 60 * 1_000;

/// `quotaCode` of a `ThrottlingException` refusing a `CreateUploadUrl` over
/// [`UPLOAD_ARTIFACT_QUOTA`].
pub const QUOTA_CODE_UPLOAD_ARTIFACTS: &str = "code-artifacts-per-account";
/// `quotaCode` of a `ThrottlingException` refusing a `CreateScan` over [`SCAN_QUOTA`].
pub const QUOTA_CODE_SCANS: &str = "scans-per-account";
/// `quotaCode` of a `ThrottlingException` refusing a `CreateScan` over
/// [`RUNS_PER_SCAN_QUOTA`].
pub const QUOTA_CODE_RUNS: &str = "revisions-per-scan";

/// How long after `CreateScan` a Standard scan run's transition out of `InProgress`
/// becomes due.
///
/// CHOSEN, not documented: AWS publishes no duration for a scan run, so this is the
/// emulator's own span for the transition the clock owes. It is never zero, so a caller
/// that reads the scan straight after `CreateScan` still observes `InProgress` and the
/// state errors that go with it.
pub const STANDARD_SCAN_SETTLE_MILLIS: i64 = 500;

/// The same span for an Express scan, which the model documents as running "on limited
/// resources and use a limited set of detectors to analyze your code in near-real time",
/// i.e. sooner than a Standard scan. Also chosen, not documented.
pub const EXPRESS_SCAN_SETTLE_MILLIS: i64 = 250;

/// Why a scan run whose code resource was never uploaded ends in `Failed`.
///
/// `CreateUploadUrl` mints an upload target and records the code artifact; the archive
/// itself arrives only when the caller performs the documented upload to that target. A
/// run started on an artifact nothing was uploaded to has no resource to analyse at all,
/// so it cannot be carried out — the one failure a run recorded here reports, and the
/// reason it reports as its `errorMessage`.
pub const SCAN_RESOURCE_NOT_UPLOADED_MESSAGE: &str =
    "The code resource for this scan run was never analyzed: nothing was uploaded to the \
     target CreateUploadUrl minted for this code artifact, so the run had no resource to \
     analyze.";

/// The first host label this emulator publishes a minted upload target under.
///
/// The label names no emulated service, so the framework's route key cannot resolve a
/// service from it: recognising the host ([`is_upload_host`]) is what routes a request to
/// a URL THIS service handed out back to this service (see `crate::service`).
pub const UPLOAD_HOST_LABEL: &str = "codeguru-security-upload";

/// How much of one uploaded archive's content this emulator keeps a copy of.
///
/// A RETENTION bound chosen by this emulator, and NOT a limit on the archive a caller may
/// upload: an upload is accepted and accounted for in full (its length and a digest over
/// every byte that arrived), and this is only how much of its content is kept alongside
/// that accounting. Nothing this service answers reads the archive back — the one question
/// a handler asks of it is whether the artifact holds a resource at all — so a bounded
/// prefix keeps the store bounded without refusing an upload the published target promises
/// to accept.
pub const UPLOAD_RETAINED_PREFIX_MAX_BYTES: usize = 64 * 1024;

/// The largest object a single `PUT` can upload, in bytes: a published upload target is a
/// pre-signed S3 URL, and S3 accepts at most 5 GB (5 x 2^30 bytes) in one `PUT`, offering
/// multipart upload above that. The published size bound of this plane, rather than a
/// bound of this emulator's choosing; see [`exceeds_single_put_maximum`].
pub const S3_SINGLE_PUT_MAX_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Whether an upload of `byte_count` bytes is larger than one `PUT` to a pre-signed S3
/// URL accepts ([`S3_SINGLE_PUT_MAX_BYTES`]). Exactly the maximum is accepted; only past
/// it is too large.
pub fn exceeds_single_put_maximum(byte_count: u64) -> bool {
    byte_count > S3_SINGLE_PUT_MAX_BYTES
}

/// The most bytes this emulator takes in ONE upload to a published target.
///
/// CHOSEN, not a published AWS limit, and a bound on this PROCESS rather than on the
/// archive a scan may be run on: the server holds a request body in memory in full before
/// any handler sees it, so an upload this emulator would have to materialise in one piece
/// has to be bounded for the process to survive it. It is therefore set far above the
/// whole zipped source trees CodeGuru Security is run on — a repository archive of tens of
/// megabytes is accepted, and so is one an order of magnitude larger — and the declared
/// length is read before the body is touched ([`upload_size_bound_exceeded`]), so reaching
/// it costs no allocation.
pub const UPLOAD_ACCEPTED_MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The size bound an upload of `byte_count` bytes is past, or `None` when it is within
/// both — the published maximum of one `PUT` ([`S3_SINGLE_PUT_MAX_BYTES`]) and what this
/// emulator takes in one upload ([`UPLOAD_ACCEPTED_MAX_BYTES`]).
///
/// Answers the bound itself, so the refusal states which one the upload is past. Exactly
/// a bound is within it; only past it is too large.
pub fn upload_size_bound_exceeded(byte_count: u64) -> Option<u64> {
    if exceeds_single_put_maximum(byte_count) {
        Some(S3_SINGLE_PUT_MAX_BYTES)
    } else if byte_count > UPLOAD_ACCEPTED_MAX_BYTES {
        Some(UPLOAD_ACCEPTED_MAX_BYTES)
    } else {
        None
    }
}

/// A code artifact `CreateUploadUrl` minted an upload target for, and `CreateScan`
/// resolves its `resourceId` against.
#[derive(Debug, Clone)]
pub struct UploadArtifact {
    /// The minted `codeArtifactId` (a UUID). Also this record's key in [`Store::upload_artifacts`].
    pub code_artifact_id: String,
    /// The `scanName` the caller said would use this resource.
    pub scan_name: String,
    /// The upload target handed back as `s3Url`.
    pub s3_url: String,
    /// The headers handed back as `requestHeaders`.
    pub request_headers: BTreeMap<String, String>,
    /// Epoch milliseconds at which the artifact was minted.
    pub created_at_millis: i64,
    /// Epoch milliseconds at which the minted target expires; see
    /// [`UPLOAD_ARTIFACT_TTL_MILLIS`].
    pub expires_at_millis: i64,
    /// The code resource uploaded to this artifact's target, once it has been, and `None`
    /// while the caller has uploaded nothing. The SINGLE copy of the uploaded bytes: a
    /// scan run resolves the resource it was started on through this record.
    pub upload: Option<UploadedResource>,
}

/// The code resource a caller uploaded to a minted target: the accounting of the archive
/// that arrived, plus a bounded prefix of its content.
///
/// The archive is not kept in full. An upload is accepted whatever its size, and what the
/// store keeps of it is its length, a digest over every byte that arrived and at most
/// [`UPLOAD_RETAINED_PREFIX_MAX_BYTES`] of its leading content — enough to identify the
/// resource a run was started on, and bounded whatever was uploaded.
#[derive(Debug, Clone)]
pub struct UploadedResource {
    /// How many bytes arrived, counted over the whole archive.
    pub byte_count: u64,
    /// Lowercase hex SHA-256 over every byte that arrived, so the resource a target holds
    /// is identified by its whole content and not only by what is retained of it.
    pub content_sha256: String,
    /// The archive's leading bytes, exactly as they arrived, truncated to
    /// [`UPLOAD_RETAINED_PREFIX_MAX_BYTES`] — the whole archive when it is no longer than
    /// that.
    pub retained_prefix: Vec<u8>,
    /// The media type the upload declared, when it declared one.
    pub content_type: Option<String>,
    /// Epoch milliseconds at which the upload was received.
    pub received_at_millis: i64,
}

impl UploadedResource {
    /// Account for the archive `bytes` that arrived at a minted target: its length, its
    /// digest and the prefix of it this store retains.
    ///
    /// Takes the bytes by reference and copies only the retained prefix, so receiving an
    /// archive costs one pass over the buffered body rather than a second copy of it.
    /// The digest and the hex rendering are both done by maintained crates this crate
    /// already depends on.
    pub fn received(bytes: &[u8], content_type: Option<String>, received_at_millis: i64) -> Self {
        use ::sha2::Digest;

        let mut hasher = ::sha2::Sha256::new();
        hasher.update(bytes);
        let retained = bytes.len().min(UPLOAD_RETAINED_PREFIX_MAX_BYTES);
        Self {
            byte_count: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            content_sha256: ::hex::encode(hasher.finalize()),
            retained_prefix: bytes[..retained].to_vec(),
            content_type,
            received_at_millis,
        }
    }
}

/// One run (one revision) of a scan.
#[derive(Debug, Clone)]
pub struct ScanRun {
    /// The `runId` (a UUID) that identifies this individual run.
    pub run_id: String,
    /// The code artifact this run was started on.
    pub code_artifact_id: String,
    /// `Express` or `Standard`, as this run was started.
    pub scan_type: String,
    /// `All` or `Security`, as this run was started.
    pub analysis_type: String,
    /// `InProgress` until [`settle_due_millis`](Self::settle_due_millis) passes, then the
    /// terminal state the run reached: `Successful` for a run that was carried out,
    /// `Failed` for one that could not be (see [`Store::settle_due_runs`]).
    pub scan_state: String,
    /// Epoch milliseconds at which the run was started.
    pub created_at_millis: i64,
    /// Epoch milliseconds of the run's last state change, absent while it has had none.
    pub updated_at_millis: Option<i64>,
    /// Why the run failed, once it has; absent on a run that has not failed, so a
    /// `Successful` run carries no `errorMessage`.
    pub error_message: Option<String>,
    /// Whether the code artifact this run was started on held an uploaded resource when
    /// the run started — i.e. whether the run had a code resource to analyse at all,
    /// which is what decides the terminal state it settles into. Decided once, at the
    /// start of the run, so what the run reports never changes under a later upload to
    /// the same target.
    pub resource_uploaded: bool,
    /// Epoch milliseconds at which this run's transition out of `InProgress` falls due.
    /// Stored, never slept on: a reader materialises the transition the clock already
    /// owes (see [`Store::settle_due_runs`]).
    pub settle_due_millis: i64,
}

/// What a `CreateScan` request asked for, as the service resolved it: every modelled
/// input of the operation, with the documented defaults already applied.
///
/// This is what one client token stands for. Two requests carrying the same token are the
/// SAME request exactly when these compare equal, so a member named explicitly with the
/// value the service would have defaulted to is the same request as one that left it out,
/// and an absent tag map is the same request as an empty one — both ask for the same work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanRequestParameters {
    /// `scanName`, the name the revisions of one resource are tracked under.
    pub scan_name: String,
    /// `resourceId.codeArtifactId`, the code artifact the run is to be started on.
    pub code_artifact_id: String,
    /// `scanType`, resolved ([`SCAN_TYPE_STANDARD`] when the request named none).
    pub scan_type: String,
    /// `analysisType`, resolved ([`ANALYSIS_TYPE_SECURITY`] when the request named none).
    pub analysis_type: String,
    /// `tags`, as the request carried them; empty when it carried no map at all.
    pub tags: BTreeMap<String, String>,
}

/// One `clientToken` record: the request the token was first seen on and the run that
/// request started, honoured until [`expires_at_millis`](Self::expires_at_millis).
///
/// `clientToken` is `@idempotencyToken`, and a retry is the SAME request sent again: the
/// record therefore keeps the whole request ([`ScanRequestParameters`]), so a replay can
/// be told apart from a different request sent under one token, rather than only the id of
/// what the first call produced.
#[derive(Debug, Clone)]
pub struct IdempotentCreateScan {
    /// The token itself. Also this record's key in [`Store::idempotency`].
    pub client_token: String,
    /// The request the token was first seen on.
    pub request: ScanRequestParameters,
    /// The `runId` that request started, which a replay of it answers with.
    pub run_id: String,
    /// Epoch milliseconds at which the token was first seen.
    pub recorded_at_millis: i64,
    /// Epoch milliseconds at which the token stops standing for that request; see
    /// [`CLIENT_TOKEN_WINDOW_MILLIS`]. Stored, never slept on: a later call materialises
    /// the expiry the clock already owes ([`Store::expire_due_client_tokens`]).
    pub expires_at_millis: i64,
}

/// The account's CodeGuru Security configuration record — the resource
/// `GetAccountConfiguration` reports and `UpdateAccountConfiguration` updates.
///
/// Its EXISTENCE is what distinguishes an account that CodeGuru Security holds a
/// configuration for from one it does not: an account with a record may be using the
/// default AWS-managed key (`encryption_kms_key_arn` absent), which is a different state
/// from having no record at all.
#[derive(Debug, Clone)]
pub struct AccountConfiguration {
    /// The account's `encryptionConfig.kmsKeyArn`: the customer-managed KMS key, or
    /// absent while the account uses the AWS-managed key ("If an AWS-managed key is used
    /// for encryption, returns empty", `EncryptionConfig$kmsKeyArn`).
    pub encryption_kms_key_arn: Option<String>,
    /// Epoch milliseconds at which the record came into existence (see
    /// [`Store::onboard_account`]).
    pub created_at_millis: i64,
}

/// A scan: the unique name CodeGuru Security tracks revisions of a resource under,
/// plus every run recorded against it.
#[derive(Debug, Clone)]
pub struct Scan {
    /// The scan's unique name. Also this record's key in [`Store::scans`].
    pub scan_name: String,
    /// `arn:aws:codeguru-security:<region>:<account>:scans/<scanName>`.
    pub scan_name_arn: String,
    /// Epoch milliseconds at which the scan was created.
    pub created_at_millis: i64,
    /// Epoch milliseconds of the scan's last change — a new revision, or a run's state
    /// transition. Absent until the scan has changed since it was created.
    pub updated_at_millis: Option<i64>,
    /// The tags `CreateScan` carried. The single tag store for this scan, so a later
    /// Tag/Untag/ListTagsForResource slice resolves the ARN to this record rather than
    /// keeping a second copy.
    pub tags: BTreeMap<String, String>,
    /// Every run, oldest first; the last one answers a `GetScan` that names no `runId`.
    /// Bounded by [`RUNS_PER_SCAN_QUOTA`].
    pub runs: Vec<ScanRun>,
}

impl Scan {
    /// The most recent run. A scan always has at least one: it is created with its
    /// first run.
    pub fn latest_run(&self) -> Option<&ScanRun> {
        self.runs.last()
    }

    /// The run `run_id` names, or the most recent run when `run_id` is absent.
    pub fn run(&self, run_id: Option<&str>) -> Option<&ScanRun> {
        match run_id {
            Some(id) => self.runs.iter().find(|run| run.run_id == id),
            None => self.latest_run(),
        }
    }

    /// The run an account-wide scan LISTING reports this scan from, or `None` when the
    /// scan is not one a listing returns at all: `ListScans` "Does not return `EXPRESS`
    /// scans", so a scan whose current run is an Express run is reported by no listing
    /// (it stays reachable by name through `GetScan`). A scan with no run has nothing to
    /// report either.
    pub fn listed_run(&self) -> Option<&ScanRun> {
        self.latest_run()
            .filter(|run| run.scan_type != SCAN_TYPE_EXPRESS)
    }

    /// `numberOfRevisions`: "The number of times a scan has been re-run on a revised
    /// resource" (`GetScanResponse$numberOfRevisions`), so the run that created the scan
    /// is not counted and a scan with one run reports `0`.
    pub fn number_of_revisions(&self) -> i64 {
        i64::try_from(self.runs.len().saturating_sub(1)).unwrap_or(i64::MAX)
    }

    /// The scan's `updatedAt` as reported for a run of `scan_type`: "Only available for
    /// `STANDARD` scan types" (`GetScanResponse$updatedAt`), so an Express run reports
    /// none.
    pub fn updated_at_for(&self, scan_type: &str) -> Option<i64> {
        if scan_type == SCAN_TYPE_STANDARD {
            self.updated_at_millis
        } else {
            None
        }
    }
}

/// Per-service state for com-amazonaws-codegurusecurity-awscodegurusecurity. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodegurusecurityAwscodegurusecurityState {
    /// Code artifacts minted by `CreateUploadUrl`, keyed by `codeArtifactId`. Bounded by
    /// [`UPLOAD_ARTIFACT_QUOTA`] and by [`UPLOAD_ARTIFACT_TTL_MILLIS`].
    pub upload_artifacts: HashMap<String, UploadArtifact>,
    /// `(expires_at_millis, codeArtifactId)` of every artifact still minted: the
    /// due-queue [`Self::expire_due_artifacts`] walks, so releasing expired artifacts
    /// costs only what actually expired.
    pub artifact_expiry: BTreeSet<(i64, String)>,
    /// Scans, keyed by `scanName`. A `BTreeMap`, so `ListScans` walks them in one
    /// stable order and a page token resumes exactly where the previous page stopped.
    /// Bounded by [`SCAN_QUOTA`].
    pub scans: BTreeMap<String, Scan>,
    /// `(settle_due_millis, scanName, run index)` of every run still `InProgress`: the
    /// due-queue [`Self::settle_due_runs`] walks, so a read materialises only the
    /// transitions that actually came due.
    pub pending_runs: BTreeSet<(i64, String, usize)>,
    /// Findings, keyed by `(scanName, findingId)` — the SINGLE store `GetFindings` and
    /// `BatchGetFindings` both read, so an id one returns resolves through the other.
    /// A run fills it with what its analysis produced; see [`SCAN_NOT_ANALYSED_MESSAGE`]
    /// for why a run this emulator records produces none.
    pub findings: BTreeMap<(String, String), Finding>,
    /// `clientToken` -> what the request carrying it asked for and the run it started, so
    /// a retried `CreateScan` answers with that run instead of starting another revision
    /// and a DIFFERENT request sent under the same token is told apart from a retry.
    /// Keyed on the token alone, because one token stands for one request: the request's
    /// own members — its `scanName` included — are compared from the record rather than
    /// folded into the key. Bounded by [`CLIENT_TOKEN_WINDOW_MILLIS`].
    pub idempotency: HashMap<String, IdempotentCreateScan>,
    /// `(expires_at_millis, clientToken)` of every token still inside its window: the
    /// due-queue [`Self::expire_due_client_tokens`] walks, so forgetting the tokens whose
    /// window has closed costs only what actually closed.
    pub idempotency_expiry: BTreeSet<(i64, String)>,
    /// How many bytes of uploaded content the resources of `upload_artifacts` retain in
    /// total, kept in step with every write and release so the figure is available
    /// without walking the store. Bounded by [`UPLOAD_ARTIFACT_QUOTA`] times
    /// [`UPLOAD_RETAINED_PREFIX_MAX_BYTES`], whatever sizes were uploaded.
    pub upload_bytes_held: usize,
    /// The account's configuration record, absent until the account is onboarded to
    /// CodeGuru Security ([`Self::onboard_account`]). The SINGLE copy of the
    /// configuration: `GetAccountConfiguration` reports it (reporting the documented
    /// AWS-managed default while it is absent), `UpdateAccountConfiguration` writes it
    /// ([`Self::set_encryption_kms_key_arn`], which brings the record into existence when
    /// the account has none) and `CreateUploadUrl` derives an upload's
    /// server-side-encryption headers from it, so there is no second copy to
    /// drift. One record, not a map: every request this emulator serves is for the one
    /// emulator account (`engine::emulator_account_id`).
    pub account_configuration: Option<AccountConfiguration>,
}

/// Shorthand for this service's state, used in the doc links above.
pub type Store = ComAmazonawsCodegurusecurityAwscodegurusecurityState;

impl ComAmazonawsCodegurusecurityAwscodegurusecurityState {
    /// Materialise every scan-run transition out of `InProgress` that the clock has
    /// already made due at `now_millis`.
    ///
    /// Each reader calls this before it answers, so a run that was owed a transition is
    /// reported in the state it was owed even if nobody polled at the moment it fell
    /// due. The transition is stamped with the instant it became DUE, not with `now`, so
    /// two reads of the same settled run report the same `updatedAt`. Driven off the
    /// `pending_runs` due-queue, so the work is proportional to what came due and not to
    /// everything stored.
    ///
    /// WHICH terminal state a run settles into is decided by what the run had to analyse,
    /// never by a fixed outcome: a run started on a code artifact whose uploaded resource
    /// this store holds was carried out, so it settles into `Successful` carrying no
    /// `errorMessage`, while a run started on an artifact nothing was uploaded to had no
    /// resource at all, so it settles into `Failed` and reports why
    /// ([`SCAN_RESOURCE_NOT_UPLOADED_MESSAGE`]) — `errorMessage` being "Details about the
    /// error that causes a scan to fail" (`GetScanResponse$errorMessage`), it belongs to
    /// the failure and to nothing else.
    pub fn settle_due_runs(&mut self, now_millis: i64) {
        let horizon = (now_millis.saturating_add(1), String::new(), 0usize);
        let due: Vec<(i64, String, usize)> =
            self.pending_runs.range(..horizon).cloned().collect();
        for key in due {
            self.pending_runs.remove(&key);
            let (due_at, scan_name, run_index) = key;
            let Some(scan) = self.scans.get_mut(&scan_name) else {
                continue;
            };
            let Some(run) = scan.runs.get_mut(run_index) else {
                continue;
            };
            if run.scan_state != SCAN_STATE_IN_PROGRESS {
                continue;
            }
            // The run settles into the state its own work reached: carried out where the
            // resource it was started on exists, failed where there was nothing to
            // analyse. `errorMessage` goes with the failure only.
            if run.resource_uploaded {
                run.scan_state = SCAN_STATE_SUCCESSFUL.to_string();
                run.error_message = None;
            } else {
                run.scan_state = SCAN_STATE_FAILED.to_string();
                run.error_message = Some(SCAN_RESOURCE_NOT_UPLOADED_MESSAGE.to_string());
            }
            run.updated_at_millis = Some(due_at);
            scan.updated_at_millis = Some(scan.updated_at_millis.unwrap_or(i64::MIN).max(due_at));
        }
    }

    /// Release every minted upload target whose lifetime the clock has already ended at
    /// `now_millis`. Driven off the `artifact_expiry` due-queue, so the work is
    /// proportional to what expired.
    pub fn expire_due_artifacts(&mut self, now_millis: i64) {
        let horizon = (now_millis.saturating_add(1), String::new());
        let due: Vec<(i64, String)> = self.artifact_expiry.range(..horizon).cloned().collect();
        for key in due {
            self.artifact_expiry.remove(&key);
            if let Some(released) = self.upload_artifacts.remove(&key.1) {
                self.release_upload_bytes(&released);
            }
        }
    }

    /// Forget every client token whose window the clock has already closed at
    /// `now_millis`, so the next `CreateScan` carrying it is a fresh request rather than a
    /// replay of one made arbitrarily long ago. Driven off the `idempotency_expiry`
    /// due-queue, so the work is proportional to what came due.
    pub fn expire_due_client_tokens(&mut self, now_millis: i64) {
        let horizon = (now_millis.saturating_add(1), String::new());
        let due: Vec<(i64, String)> = self.idempotency_expiry.range(..horizon).cloned().collect();
        for key in due {
            self.idempotency_expiry.remove(&key);
            if self
                .idempotency
                .get(&key.1)
                .is_some_and(|record| record.expires_at_millis <= now_millis)
            {
                self.idempotency.remove(&key.1);
            }
        }
    }

    /// Record what one client token stands for, registering the close of its window on the
    /// due-queue. A token already recorded keeps one record only: its previous due-queue
    /// entry goes with it, so the queue and the ledger stay in step.
    pub fn remember_client_token(&mut self, record: IdempotentCreateScan) {
        let key = (record.expires_at_millis, record.client_token.clone());
        if let Some(previous) = self.idempotency.insert(record.client_token.clone(), record) {
            self.idempotency_expiry
                .remove(&(previous.expires_at_millis, previous.client_token));
        }
        self.idempotency_expiry.insert(key);
    }

    /// Take the bytes a released artifact retained out of the retained total.
    fn release_upload_bytes(&mut self, released: &UploadArtifact) {
        if let Some(upload) = &released.upload {
            self.upload_bytes_held = self
                .upload_bytes_held
                .saturating_sub(upload.retained_prefix.len());
        }
    }

    /// Receive one upload to a minted target: hold `received` as the code resource of the
    /// artifact `target` names.
    ///
    /// The whole judgement lives here, so the data plane that serves the target and any
    /// later caller agree on what a target holds:
    ///
    /// * every part of the published URL must agree with the recorded artifact — an
    ///   artifact this store no longer holds (its lifetime ended), or a path that crosses
    ///   one artifact's id with another scan's name or another account, names no target;
    /// * the archive is accepted whatever its size — what is held of it is the accounting
    ///   [`UploadedResource::received`] already computed, so a large upload is stored
    ///   rather than refused;
    /// * a target holds ONE resource, so a second upload to the same target replaces the
    ///   first — which is what makes an interrupted upload retryable.
    ///
    /// Takes the accounting already computed, so the per-upload work that is proportional
    /// to the archive (the digest, the retained copy) happens OUTSIDE this lock and only
    /// the store's own bookkeeping runs inside it, whatever was uploaded.
    pub fn receive_upload(
        &mut self,
        target: &UploadTarget,
        received: UploadedResource,
    ) -> UploadOutcome {
        // The targets whose lifetime the clock has already ended are released first, so an
        // expired target is not written to.
        self.expire_due_artifacts(received.received_at_millis);
        let held = self
            .upload_artifacts
            .get(&target.code_artifact_id)
            .filter(|artifact| artifact.scan_name == target.scan_name)
            .filter(|_| target.account_id == ::engine::emulator_account_id());
        let Some(artifact) = held else {
            return UploadOutcome::NoSuchTarget;
        };
        let already = artifact
            .upload
            .as_ref()
            .map_or(0, |held| held.retained_prefix.len());
        let retains = received.retained_prefix.len();
        let Some(artifact) = self.upload_artifacts.get_mut(&target.code_artifact_id) else {
            return UploadOutcome::NoSuchTarget;
        };
        artifact.upload = Some(received);
        self.upload_bytes_held = self
            .upload_bytes_held
            .saturating_sub(already)
            .saturating_add(retains);
        UploadOutcome::Stored
    }

    /// The account's configured customer-managed `encryptionConfig.kmsKeyArn`: the key
    /// of the account's configuration record, and none when the account has no record or
    /// its record names no key (both are the documented AWS-managed default — "By
    /// default, CodeGuru Security uses an AWS-managed key for encryption",
    /// `GetAccountConfigurationResponse$encryptionConfig`).
    pub fn encryption_kms_key_arn(&self) -> Option<String> {
        self.account_configuration
            .as_ref()
            .and_then(|config| config.encryption_kms_key_arn.clone())
    }

    /// Bring the account's configuration record into existence, if it does not exist yet,
    /// as the account's first CodeGuru Security resource does.
    ///
    /// CodeGuru Security holds an account configuration for an account that is ONBOARDED
    /// to the service, and its API has no onboarding operation: the record appears when
    /// the account starts using the service. This emulator therefore creates it on the
    /// first call that writes state for the account (`CreateUploadUrl`, `CreateScan`,
    /// `UpdateAccountConfiguration`), and a newly created record names no customer-managed
    /// key, so the account keeps using the AWS-managed key until an update names one.
    ///
    /// IDEMPOTENT: an account that already has a record keeps it, with the key it
    /// configured and the instant it was created.
    pub fn onboard_account(&mut self, now_millis: i64) {
        if self.account_configuration.is_none() {
            self.account_configuration = Some(AccountConfiguration {
                encryption_kms_key_arn: None,
                created_at_millis: now_millis,
            });
        }
    }

    /// Write the account's `encryptionConfig.kmsKeyArn` — `None` asks for the AWS-managed
    /// key — and report the value now configured.
    ///
    /// The account's encryption configuration is a per-account SETTING with a documented
    /// default, not a named resource a caller creates: writing it therefore brings the
    /// record into existence for an account that has none — the same record
    /// [`Self::onboard_account`] creates — rather than failing, so the setting can be
    /// configured as the account's first call. The write itself cannot be skipped: the
    /// record is obtained, created if need be, and then written through.
    pub fn set_encryption_kms_key_arn(
        &mut self,
        kms_key_arn: Option<String>,
        now_millis: i64,
    ) -> Option<String> {
        let config = self
            .account_configuration
            .get_or_insert(AccountConfiguration {
                encryption_kms_key_arn: None,
                created_at_millis: now_millis,
            });
        config.encryption_kms_key_arn = kms_key_arn;
        config.encryption_kms_key_arn.clone()
    }

    /// Record a minted upload target, registering its expiry.
    pub fn record_artifact(&mut self, artifact: UploadArtifact) {
        self.artifact_expiry
            .insert((artifact.expires_at_millis, artifact.code_artifact_id.clone()));
        self.upload_artifacts
            .insert(artifact.code_artifact_id.clone(), artifact);
    }

    /// Register a run's clock-owed transition on the due-queue.
    pub fn track_pending_run(&mut self, scan_name: &str, run_index: usize, due_millis: i64) {
        self.pending_runs
            .insert((due_millis, scan_name.to_string(), run_index));
    }

    /// Every stored finding of `scan_name`, in `findingId` order. Reached by the
    /// `(scanName, findingId)` key order, so the walk covers only this scan's entries.
    pub fn findings_of(&self, scan_name: &str) -> Vec<(String, Finding)> {
        self.findings
            .range((scan_name.to_string(), String::new())..)
            .take_while(|((name, _), _)| name == scan_name)
            .map(|((_, finding_id), finding)| (finding_id.clone(), finding.clone()))
            .collect()
    }

    /// The stored finding `finding_id` of `scan_name` names.
    pub fn finding(&self, scan_name: &str, finding_id: &str) -> Option<&Finding> {
        self.findings
            .get(&(scan_name.to_string(), finding_id.to_string()))
    }

    /// The scan the resource ARN `arn` names, for the tagging operations.
    ///
    /// An ARN names a resource of one region and one account, so the stored scan
    /// answers only when the ARN it was created with is the ARN the request named: an
    /// otherwise well-formed ARN naming another region or another account names no
    /// scan of this account, and is reported as absent.
    pub fn scan_by_arn(&self, arn: &str) -> Option<&Scan> {
        let parsed = parse_scan_arn(arn)?;
        self.scans
            .get(parsed.scan_name)
            .filter(|scan| scan.scan_name_arn == arn)
    }

    /// [`Self::scan_by_arn`] for the tagging operations that write the scan's tags.
    pub fn scan_by_arn_mut(&mut self, arn: &str) -> Option<&mut Scan> {
        let parsed = parse_scan_arn(arn)?;
        let scan = self.scans.get_mut(parsed.scan_name)?;
        (scan.scan_name_arn == arn).then_some(scan)
    }
}

/// Epoch milliseconds now. The one clock this service reads, so every timestamp it
/// stores is on one scale.
pub fn now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| i64::try_from(elapsed.as_millis()).ok())
        .unwrap_or_default()
}

/// Render stored epoch milliseconds as the modelled `Timestamp`.
pub fn to_timestamp(epoch_millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs_f64(epoch_millis as f64 / 1000.0)
}

/// Read a modelled `Timestamp` as epoch milliseconds, the scale every instant this
/// service stores is on. The inverse of [`to_timestamp`], so an instant that went out
/// on the wire comes back on the same scale.
pub fn from_timestamp(at: &::aws_smithy_types::DateTime) -> i64 {
    at.to_millis()
        .unwrap_or_else(|_| at.secs().saturating_mul(1_000))
}

/// The start of the UTC day `epoch_millis` falls in: the instant itself floored to a
/// whole multiple of [`MILLIS_PER_DAY`]. `div_euclid`, so an instant before the epoch
/// floors to the start of ITS day rather than towards the epoch.
pub fn start_of_utc_day_millis(epoch_millis: i64) -> i64 {
    epoch_millis
        .div_euclid(MILLIS_PER_DAY)
        .saturating_mul(MILLIS_PER_DAY)
}

/// The last instant of the UTC day `epoch_millis` falls in — the instant a day's
/// metrics are reported "as of" (`MetricsSummary`: "A summary of metrics for an account
/// as of a specified date").
pub fn end_of_utc_day_millis(epoch_millis: i64) -> i64 {
    start_of_utc_day_millis(epoch_millis).saturating_add(MILLIS_PER_DAY - 1)
}

/// Whether the UTC day that begins at `day_start_millis` lies inside the window of
/// `window_days` days that ENDS with the UTC day `now_millis` falls in.
///
/// The ONE place a day-bounded input constraint is decided, so every member the model
/// documents as having to fall within a bounded span back from today is checked the same
/// way: a day later than today is outside the window (it has not happened yet), and so
/// is a day more than `window_days` days before today. Both bounds are whole UTC days,
/// because a value the model rounds to the nearest day names a day, not an instant — so
/// any instant of today is inside the window and any instant of tomorrow is outside it.
pub fn day_within_past_window(day_start_millis: i64, now_millis: i64, window_days: i64) -> bool {
    let earliest = start_of_utc_day_millis(
        now_millis.saturating_sub(window_days.saturating_mul(MILLIS_PER_DAY)),
    );
    let today = start_of_utc_day_millis(now_millis);
    (earliest..=today).contains(&day_start_millis)
}

/// The bucket of `FindingMetricsValuePerSeverity` a finding's `severity` is counted in,
/// or `None` when the finding records no modelled `Severity` value.
///
/// Shared by both metrics operations (`GetMetricsSummary` and `ListFindingsMetrics`), so
/// one definition decides which severity a stored finding is counted under.
pub fn severity_bucket(severity: Option<&str>) -> Option<&'static str> {
    match severity? {
        SEVERITY_INFO => Some(SEVERITY_INFO),
        SEVERITY_LOW => Some(SEVERITY_LOW),
        SEVERITY_MEDIUM => Some(SEVERITY_MEDIUM),
        SEVERITY_HIGH => Some(SEVERITY_HIGH),
        SEVERITY_CRITICAL => Some(SEVERITY_CRITICAL),
        _ => None,
    }
}

/// A `FindingMetricsValuePerSeverity` whose members are what `value_of` answers for each
/// modelled `Severity`. The ONE place the mapping from a severity to its member lives,
/// so every per-severity metric of this service reports the same five buckets.
pub fn per_severity_with(
    mut value_of: impl FnMut(&'static str) -> Option<f64>,
) -> FindingMetricsValuePerSeverity {
    FindingMetricsValuePerSeverity {
        info: value_of(SEVERITY_INFO),
        low: value_of(SEVERITY_LOW),
        medium: value_of(SEVERITY_MEDIUM),
        high: value_of(SEVERITY_HIGH),
        critical: value_of(SEVERITY_CRITICAL),
    }
}

/// `FindingMetricsValuePerSeverity` for a COUNT per severity: every severity is
/// reported, and a severity nothing was counted for reports `0` rather than being left
/// absent — zero findings of a severity is a measurement, not a missing one.
pub fn per_severity_value(counts: &BTreeMap<&'static str, i64>) -> FindingMetricsValuePerSeverity {
    per_severity_with(|severity| Some(counts.get(severity).copied().unwrap_or(0) as f64))
}

/// The region this request is for, read from its SigV4 credential scope.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// `scanNameArn` for `scan_name`:
/// `arn:aws:codeguru-security:<region>:<account>:scans/<scanName>`, built through the
/// shared ARN helper so the account segment is the emulator's canonical one.
pub fn scan_name_arn(region: &str, scan_name: &str) -> String {
    ::engine::emulator_arn(SERVICE_CODE, region, &format!("scans/{scan_name}"))
}

/// The upload target `CreateUploadUrl` publishes as `s3Url` for one code artifact:
/// `https://<UPLOAD_HOST_LABEL>.<region>.amazonaws.com/<account>/<scanName>/<artifactId>`.
///
/// A URL this emulator SERVES: the host is the one [`is_upload_host`] recognises and the
/// path is the one [`parse_upload_path`] reads back, so the upload the response documents
/// can actually be performed against this emulator (`crate::service` serves it).
///
/// The URL is derived from the RECORDED artifact. The region only ever reaches the host
/// through [`url_region`], which keeps a credential scope that is not a Region out of the
/// host position; the scan name and the artifact id are path segments of a record this
/// service minted.
pub fn upload_url(region: &str, scan_name: &str, code_artifact_id: &str) -> String {
    format!(
        "https://{host}/{account}/{scan_name}/{code_artifact_id}",
        host = upload_host(region),
        account = ::engine::emulator_account_id(),
    )
}

/// The host [`upload_url`] publishes a target on, for `region`.
pub fn upload_host(region: &str) -> String {
    format!(
        "{UPLOAD_HOST_LABEL}.{region}.amazonaws.com",
        region = url_region(region)
    )
}

/// True when `host` is the hostname this service publishes its upload targets under, as
/// [`upload_host`] builds it — with any `user@` and `:port` removed, as a `Host` header
/// may carry either.
///
/// Recognising it is what routes a request to a URL this service HANDED OUT back to this
/// service: the host's first label names no service, so the framework's route key cannot
/// name one from it.
pub fn is_upload_host(host: &str) -> bool {
    let host = host.rsplit('@').next().unwrap_or(host);
    let host = host.split(':').next().unwrap_or(host);
    let Some(rest) = host
        .strip_prefix(UPLOAD_HOST_LABEL)
        .and_then(|rest| rest.strip_prefix('.'))
    else {
        return false;
    };
    rest.strip_suffix(".amazonaws.com")
        .is_some_and(is_region_shaped)
}

/// What a request to a published upload target names: the account the target belongs to,
/// the scan the uploaded resource is for, and the code artifact it is held against.
///
/// The path is the one [`upload_url`] builds: `/<account>/<scanName>/<codeArtifactId>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTarget {
    /// The ARN-style account id of the target's first path segment.
    pub account_id: String,
    /// The `scanName` the target was minted for.
    pub scan_name: String,
    /// The `codeArtifactId` the target was minted for.
    pub code_artifact_id: String,
}

/// The upload target a request path names, or `None` when the path is not one this
/// service publishes.
///
/// Reading the path back is a syntax check only: whether the target it names is a
/// recorded artifact of this account is [`Store::receive_upload`]'s judgement, which is
/// the one place that resolves a target against the store.
pub fn parse_upload_path(path: &str) -> Option<UploadTarget> {
    let mut segments = path.strip_prefix('/')?.split('/');
    let (Some(account_id), Some(scan_name), Some(code_artifact_id), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return None;
    };
    if account_id.is_empty() || scan_name.is_empty() || code_artifact_id.is_empty() {
        return None;
    }
    Some(UploadTarget {
        account_id: account_id.to_string(),
        scan_name: scan_name.to_string(),
        code_artifact_id: code_artifact_id.to_string(),
    })
}

/// How a write to a minted upload target was answered; see [`Store::receive_upload`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UploadOutcome {
    /// The resource is now held against the code artifact the target names.
    Stored,
    /// The target names no code artifact of this account, or no longer does.
    NoSuchTarget,
}

/// The region a published URL's host names: the request's own when it has a Region's
/// shape, else [`DEFAULT_REGION`].
///
/// A credential scope is request-controlled text, and this value is interpolated into a
/// host, so only a value shaped like a Region may reach it — which is also what keeps
/// every URL this service publishes one [`is_upload_host`] recognises.
fn url_region(region: &str) -> &str {
    if is_region_shaped(region) {
        region
    } else {
        DEFAULT_REGION
    }
}

/// True when `value` has an AWS Region's shape: lowercase alphabetic segments joined by
/// `-`, ending in a digit group, at least three segments.
fn is_region_shaped(value: &str) -> bool {
    let mut segments = value.split('-').rev();
    let Some(number) = segments.next() else {
        return false;
    };
    if number.is_empty() || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return false;
    }
    let words: Vec<&str> = segments.collect();
    words.len() >= 2
        && words
            .iter()
            .all(|word| !word.is_empty() && word.bytes().all(|byte| byte.is_ascii_lowercase()))
}

/// The `@pattern` of the modelled `ScanNameArn` shape, verbatim
/// (`smithy_query(query="shape",shape="ScanNameArn")`).
///
/// Every INPUT member of this service constrained by it is a tagging operation's
/// `resourceArn` @httpLabel — `ListTagsForResource`, `TagResource` and
/// `UntagResource` are the only operations that take a `ScanNameArn` — so a value
/// that failed THIS pattern is a resource ARN that is not an ARN. `crate::service`
/// keys the refusal it answers for such a label on this constant.
pub const SCAN_NAME_ARN_PATTERN: &str =
    r"^arn:aws:codeguru-security:[\S]+:[\d]{12}:scans\/[a-zA-Z0-9-_$:.]*$";

/// The fixed head of every `ScanNameArn`: the `aws` partition and this service's
/// namespace, as [`SCAN_NAME_ARN_PATTERN`] spells them.
pub const SCAN_ARN_PREFIX: &str = "arn:aws:codeguru-security:";

/// What separates an ARN's account-id segment from the scan name it names, from the
/// same pattern (`:scans\/`).
const SCAN_ARN_RESOURCE_SEGMENT: &str = ":scans/";

/// Digits in an ARN's account-id segment, from the same pattern (`[\d]{12}`).
const ACCOUNT_ID_DIGITS: usize = 12;

/// A `ScanNameArn` taken apart: which region and account it names, and the scan name
/// it identifies there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanArn<'a> {
    /// The ARN's region segment.
    pub region: &'a str,
    /// The ARN's account-id segment, always 12 digits.
    pub account_id: &'a str,
    /// The scan name after `scans/`; the pattern allows it to be empty.
    pub scan_name: &'a str,
}

/// Parse `arn` as a `ScanNameArn`, or `None` when it is not one.
///
/// Accepts exactly the strings [`SCAN_NAME_ARN_PATTERN`] accepts, except for the
/// shape's separate `@length` bound, which is a size limit rather than a question of
/// syntax: the prefix is fixed, the region is one or more non-whitespace characters,
/// the account id is 12 digits and the scan name holds only the characters the
/// pattern's last class allows.
///
/// The region class (`[\S]+`) can itself hold `:` and the scan-name class can hold
/// `:scans/`-shaped text, so one string may be splittable in several ways. The LAST
/// split that holds is the answer, which is where the pattern's greedy region leaves
/// the boundary.
pub fn parse_scan_arn(arn: &str) -> Option<ScanArn<'_>> {
    let rest = arn.strip_prefix(SCAN_ARN_PREFIX)?;
    let mut parsed = None;
    let mut from = 0usize;
    while let Some(offset) = rest[from..].find(SCAN_ARN_RESOURCE_SEGMENT) {
        let at = from + offset;
        let head = &rest[..at];
        let scan_name = &rest[at + SCAN_ARN_RESOURCE_SEGMENT.len()..];
        // The account id holds no `:`, so the last `:` of the head is always the one
        // between the region and the account.
        if let Some((region, account_id)) = head.rsplit_once(':') {
            if is_arn_region(region) && is_account_id(account_id) && is_scan_name(scan_name) {
                parsed = Some(ScanArn {
                    region,
                    account_id,
                    scan_name,
                });
            }
        }
        // `:scans/` cannot overlap itself, so the next candidate starts after this one.
        from = at + SCAN_ARN_RESOURCE_SEGMENT.len();
    }
    parsed
}

/// `[\S]+`: one or more non-whitespace characters.
fn is_arn_region(region: &str) -> bool {
    !region.is_empty() && !region.chars().any(char::is_whitespace)
}

/// `[\d]{12}`.
fn is_account_id(account_id: &str) -> bool {
    account_id.len() == ACCOUNT_ID_DIGITS && account_id.bytes().all(|b| b.is_ascii_digit())
}

/// `[a-zA-Z0-9-_$:.]*`.
fn is_scan_name(scan_name: &str) -> bool {
    scan_name.chars().all(is_scan_name_char)
}

/// One character of `[a-zA-Z0-9-_$:.]`, the class both [`SCAN_NAME_ARN_PATTERN`] and the
/// modelled `ScanName` shape spell for a scan name.
fn is_scan_name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '$' | ':' | '.')
}

/// The `@length` bounds of the modelled `ScanName` shape, in characters
/// (`smithy_query(query="shape",shape="ScanName")`: length 1..140, pattern
/// `^[a-zA-Z0-9-_$:.]*$`).
///
/// This is the service's own notion of a name that IS a scan name: every INPUT member
/// that takes one targets `ScanName`, so the request boundary enforces it before a
/// handler ever sees the value. An operation that takes a scan name as an unconstrained
/// `String` — `FindingIdentifier$scanName` is the only one — has to apply the same
/// notion itself, which is what [`is_valid_scan_name`] is for.
pub const SCAN_NAME_MIN_LEN: usize = 1;
/// The upper `@length` bound of `ScanName`; see [`SCAN_NAME_MIN_LEN`].
pub const SCAN_NAME_MAX_LEN: usize = 140;

/// The character [`reportable_scan_name`] reports in place of one the `ScanName` pattern
/// does not admit. Inside the pattern's class, so the reduced name is always a value the
/// constrained shape accepts.
const SCAN_NAME_FILLER: char = '_';

/// True when `value` is a scan name of this service: the modelled `ScanName` shape's
/// `@length` and `@pattern` both hold (see [`SCAN_NAME_MIN_LEN`]).
pub fn is_valid_scan_name(value: &str) -> bool {
    let length = value.chars().count();
    (SCAN_NAME_MIN_LEN..=SCAN_NAME_MAX_LEN).contains(&length) && is_scan_name(value)
}

/// `value` as a member whose target shape is `ScanName` may report it.
///
/// A valid scan name is reported verbatim. One that is not is reduced to the constrained
/// shape — each character the pattern does not admit replaced by [`SCAN_NAME_FILLER`],
/// the result cut to [`SCAN_NAME_MAX_LEN`] characters and, where the value was empty,
/// reported as the filler alone (the shape's minimum length is 1) — so a response never
/// carries a value its own member's shape forbids. The verbatim value belongs in the
/// unconstrained `message` beside it, which is where the caller reads what was rejected.
pub fn reportable_scan_name(value: &str) -> String {
    if is_valid_scan_name(value) {
        return value.to_string();
    }
    let reduced: String = value
        .chars()
        .take(SCAN_NAME_MAX_LEN)
        .map(|c| if is_scan_name_char(c) { c } else { SCAN_NAME_FILLER })
        .collect();
    if reduced.is_empty() {
        return SCAN_NAME_FILLER.to_string();
    }
    reduced
}

/// The hex-digit group widths of the modelled `Uuid` pattern, in order
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`, from
/// `smithy_query(query="shape",shape="Uuid")`).
const UUID_GROUP_WIDTHS: [usize; 5] = [8, 4, 4, 4, 12];

/// True when `value` has the form of an identifier this service hands out.
///
/// The model gives every server-generated identifier of this service the `Uuid` shape
/// (`runId`, `codeArtifactId`), and [`mint_uuid`] is the single place one is minted, so a
/// value outside that form is not an identifier this service ever issued. `findingId` is
/// documented as such an issued value (`FindingIdentifier$findingId`: "You retrieve the
/// findingId when you call GetFindings") while carrying no constraint of its own, so a
/// handler that is handed one judges its form with this.
pub fn is_minted_identifier(value: &str) -> bool {
    let mut groups = value.split('-');
    for width in UUID_GROUP_WIDTHS {
        let Some(group) = groups.next() else {
            return false;
        };
        if group.len() != width || !group.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return false;
        }
    }
    groups.next().is_none()
}

/// Mint a server-generated identifier in the textual form the `Uuid` shape requires
/// (`^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$`), for a `runId` or
/// a `codeArtifactId`.
///
/// The 128 bits come from a SHA-256 digest (the `sha2` crate) of a seed that is unique
/// to this process and this call — a monotonic counter, the clock in nanoseconds and the
/// process id — so no two calls, in one process or in two started the same nanosecond,
/// can mint the same id. The hashing and the hex rendering are both done by maintained
/// crates this workspace already depends on.
pub fn mint_uuid() -> String {
    use ::sha2::Digest;

    static MINTED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = MINTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default();

    let mut hasher = ::sha2::Sha256::new();
    hasher.update(sequence.to_be_bytes());
    hasher.update(nanos.to_be_bytes());
    hasher.update(std::process::id().to_be_bytes());
    let digest = ::hex::encode(&hasher.finalize()[..16]);

    format!(
        "{}-{}-{}-{}-{}",
        &digest[0..8],
        &digest[8..12],
        &digest[12..16],
        &digest[16..20],
        &digest[20..32],
    )
}

/// Encode a page cursor as a `nextToken`.
///
/// The modelled `NextToken` pattern is `^[\S]+$`, which the value a page resumes at
/// need not satisfy on its own (`findingId` carries no modelled constraint at all), so
/// the cursor is hex-encoded: the token is always whitespace-free, always non-empty for
/// a non-empty cursor, and opaque to the caller.
pub fn encode_page_token(cursor: &str) -> String {
    ::hex::encode(cursor)
}

/// Decode a `nextToken` built by [`encode_page_token`]. `None` when the token was not
/// built by this service, which the reader refuses as a `ValidationException`.
pub fn decode_page_token(token: &str) -> Option<String> {
    ::hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// How long after `CreateScan` a run of `scan_type` is owed its transition out of
/// `InProgress`.
pub fn settle_span_millis(scan_type: &str) -> i64 {
    if scan_type == SCAN_TYPE_EXPRESS {
        EXPRESS_SCAN_SETTLE_MILLIS
    } else {
        STANDARD_SCAN_SETTLE_MILLIS
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodegurusecurityAwscodegurusecurityState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodegurusecurityAwscodegurusecurityState>)
}
