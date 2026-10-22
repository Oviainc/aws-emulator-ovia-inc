// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codegurusecurity-awscodegurusecurity service.
//!
//! ONE store behind [`service_state`], shared by every handler of this service: the
//! upload artifacts `CreateUploadUrl` mints, the scans `CreateScan` records (each with
//! its runs), the findings both `GetFindings` and `BatchGetFindings` read, and the
//! idempotency ledger that makes a `CreateScan` retry answer with its original run.
//!
//! # Every caller-driven collection here is BOUNDED
//!
//! Each store a request can grow carries an explicit maximum, enforced in the handler
//! that writes it, so no sequence of requests can grow the emulator's heap without end:
//!
//! * [`UPLOAD_ARTIFACT_QUOTA`] caps `upload_artifacts`, and an artifact also expires
//!   [`UPLOAD_ARTIFACT_TTL_MILLIS`] after it was minted ([`Store::expire_due_artifacts`]).
//! * [`SCAN_QUOTA`] caps `scans`, and [`RUNS_PER_SCAN_QUOTA`] caps the runs of one scan,
//!   so the (scans x runs) product is bounded.
//! * `idempotency` gains one entry only where a run was created, so it is bounded by the
//!   same (scans x runs) product.
//! * `findings` is written only by a scan run's analysis, which this emulator does not
//!   perform (see [`SCAN_NOT_ANALYSED_MESSAGE`]), so no request path grows it; the
//!   readers' page sizes are clamped to the modelled `maxResults` maxima.
//!
//! Per-request CPU is bounded too: the clock-owed work the readers do is driven by the
//! `pending_runs` / `artifact_expiry` due-queues, so each call touches only the entries
//! that actually came due rather than walking everything earlier requests stored.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use crate::ops::types::Finding;

/// The region an emulated request is for: the region of its SigV4 credential scope,
/// or the emulator's documented default when the request carries no scope (see
/// `emulator_server::emulator_arn`, "emulators default to `us-east-1`").
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ScanState`, from `types::shared_types::SCAN_STATE_VALUES`
/// (`Failed | InProgress | Successful`).
pub const SCAN_STATE_IN_PROGRESS: &str = "InProgress";
/// `ScanState`; see [`SCAN_STATE_IN_PROGRESS`].
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

/// `ErrorCode`, from `types::shared_types::ERROR_CODE_VALUES`. `BatchGetFindings`
/// classifies each identifier it cannot resolve with one of these.
pub const ERROR_CODE_INVALID_SCAN_NAME: &str = "INVALID_SCAN_NAME";
/// `ErrorCode`; see [`ERROR_CODE_INVALID_SCAN_NAME`].
pub const ERROR_CODE_ITEM_DOES_NOT_EXIST: &str = "ITEM_DOES_NOT_EXIST";
/// `ErrorCode`; see [`ERROR_CODE_INVALID_SCAN_NAME`].
pub const ERROR_CODE_DUPLICATE_IDENTIFIER: &str = "DUPLICATE_IDENTIFIER";

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

/// The most runs (revisions) one scan keeps; see [`UPLOAD_ARTIFACT_QUOTA`]. Together
/// with [`SCAN_QUOTA`] this bounds the total number of runs, and with it the size of the
/// idempotency ledger and the work the due-queues can hold.
pub const RUNS_PER_SCAN_QUOTA: usize = 100;

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

/// Why every scan run this emulator records ends in `Failed` rather than `Successful`.
///
/// `CreateUploadUrl` mints the upload target and records the code artifact, but this
/// emulator has no S3 data plane to accept the upload and does not run CodeGuru
/// Security's detectors, so the analysis a scan run represents never happens. The run
/// therefore ends in the modelled failure state carrying this reason, instead of
/// reporting `Successful` for work nothing performed.
pub const SCAN_NOT_ANALYSED_MESSAGE: &str =
    "The code resource for this scan run was never analyzed: this emulator mints the \
     CreateUploadUrl upload target but has no S3 data plane to receive the resource and \
     does not run CodeGuru Security's detectors, so the run produced no findings.";

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
    /// `InProgress` until [`settle_due_millis`](Self::settle_due_millis) passes, then
    /// `Failed`.
    pub scan_state: String,
    /// Epoch milliseconds at which the run was started.
    pub created_at_millis: i64,
    /// Epoch milliseconds of the run's last state change, absent while it has had none.
    pub updated_at_millis: Option<i64>,
    /// Why the run failed, once it has.
    pub error_message: Option<String>,
    /// Epoch milliseconds at which this run's transition out of `InProgress` falls due.
    /// Stored, never slept on: a reader materialises the transition the clock already
    /// owes (see [`Store::settle_due_runs`]).
    pub settle_due_millis: i64,
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
    /// `(clientToken, scanName)` -> `runId`, so a retried `CreateScan` answers with the
    /// run the first call created instead of starting another revision. Keyed on the
    /// scan name as well as the token, so a token reused for a different scan can never
    /// answer for the other scan's run.
    pub idempotency: HashMap<(String, String), String>,
    /// The account's `encryptionConfig.kmsKeyArn`, which decides the server-side
    /// encryption headers `CreateUploadUrl` requires for an upload. Absent until
    /// `UpdateAccountConfiguration` sets one — "By default, CodeGuru Security uses an
    /// AWS-managed key for encryption" (`GetAccountConfigurationResponse$encryptionConfig`)
    /// — and absent again once an update passes an `encryptionConfig` with no key. The
    /// SINGLE copy of the configuration: `GetAccountConfiguration` reports it and
    /// `CreateUploadUrl` derives an upload's encryption headers from it, so there is no
    /// second copy to drift.
    pub encryption_kms_key_arn: Option<String>,
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
            run.scan_state = SCAN_STATE_FAILED.to_string();
            run.error_message = Some(SCAN_NOT_ANALYSED_MESSAGE.to_string());
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
            self.upload_artifacts.remove(&key.1);
        }
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
