// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-glacier-glacier service, plus the helpers more
//! than one handler needs to read it back onto the wire.
//!
//! `src/lib.rs` is generated and declares exactly four modules, so this is the one place
//! in the crate where logic shared by several operations can live: the vault/archive/job
//! records, the SHA-256 tree hash, the ISO-8601 clock, and the `DescribeVaultOutput` /
//! `GlacierJobDescription` renderers that `DescribeVault`/`ListVaults` and
//! `DescribeJob`/`ListJobs` must agree on.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use sha2::{Digest, Sha256};

use crate::ops::types::{
    DataRetrievalPolicy, DescribeJobOutput, DescribeVaultOutput, InvalidParameterValueException,
    InventoryRetrievalJobDescription, LimitExceededException, MissingParameterValueException,
    PolicyEnforcedException, ProvisionedCapacityDescription, ResourceNotFoundException,
    ServiceUnavailableException, VaultNotificationConfig,
};

/// `ActionCode` for an archive retrieval job, as `ACTION_CODE_VALUES` spells it.
pub const ACTION_ARCHIVE_RETRIEVAL: &str = "ArchiveRetrieval";
/// `ActionCode` for an inventory retrieval job, as `ACTION_CODE_VALUES` spells it.
pub const ACTION_INVENTORY_RETRIEVAL: &str = "InventoryRetrieval";

/// `StatusCode` for a job that has not finished yet, as `STATUS_CODE_VALUES` spells it.
pub const STATUS_IN_PROGRESS: &str = "InProgress";
/// `StatusCode` for a job whose output is ready, as `STATUS_CODE_VALUES` spells it.
pub const STATUS_SUCCEEDED: &str = "Succeeded";

/// `JobParameters$Type` values the model documents:
/// "Valid values are \"select\", \"archive-retrieval\" and \"inventory-retrieval\"."
pub const JOB_TYPE_ARCHIVE_RETRIEVAL: &str = "archive-retrieval";
/// See [`JOB_TYPE_ARCHIVE_RETRIEVAL`].
pub const JOB_TYPE_INVENTORY_RETRIEVAL: &str = "inventory-retrieval";
/// See [`JOB_TYPE_ARCHIVE_RETRIEVAL`].
pub const JOB_TYPE_SELECT: &str = "select";

/// `JobParameters$Tier` values the model documents: "Valid values are `Expedited`,
/// `Standard`, or `Bulk`. `Standard` is the default."
pub const TIER_VALUES: &[&str] = &["Bulk", "Expedited", "Standard"];
/// The tier the model documents as the default when the request omits one.
pub const DEFAULT_TIER: &str = "Standard";

/// `JobParameters$Format` values the model documents for an inventory retrieval:
/// "Valid values are \"CSV\" and \"JSON\"."
pub const INVENTORY_FORMAT_VALUES: &[&str] = &["CSV", "JSON"];
/// The inventory format the model documents as the default: "If you are initiating an
/// inventory job and do not specify a Format field, JSON is the default format."
pub const DEFAULT_INVENTORY_FORMAT: &str = "JSON";

/// The megabyte the SHA-256 tree hash is built over, and the unit a
/// `RetrievalByteRange` must be aligned to: "the byte range must be megabyte (1024*1024)
/// aligned" (`JobParameters$RetrievalByteRange`).
pub const ONE_MEGABYTE: u64 = 1024 * 1024;

/// `ListVaultsInput$limit`: "The default limit is 10."
pub const DEFAULT_LIST_VAULTS_LIMIT: i32 = 10;
/// `ListJobsInput$limit`: "The default limit is 50."
pub const DEFAULT_LIST_JOBS_LIMIT: i32 = 50;

/// `CreateVault`: "You can create up to 1,000 vaults per account."
pub const MAX_VAULTS_PER_ACCOUNT: usize = 1000;

/// `CreateVault`: "Names can be between 1 and 255 characters long."
pub const MIN_VAULT_NAME_LEN: usize = 1;
/// See [`MIN_VAULT_NAME_LEN`].
pub const MAX_VAULT_NAME_LEN: usize = 255;

/// How long after `InitiateJob` a retrieval job's output becomes available.
///
/// A retrieval job is reported `InProgress` by `InitiateJob`, so the move to
/// `Succeeded` is a transition the clock owes. The span is deliberately non-zero —
/// a caller that polls `DescribeJob` straight after `InitiateJob` must still observe
/// `InProgress` and `GetJobOutput`'s refusal — and deliberately short, because the
/// hours real Glacier takes are not worth emulating.
pub const JOB_COMPLETION_DELAY: Duration = Duration::from_millis(500);

/// How long a completed job, and the output it holds open, is kept.
///
/// `GetJobOutput`: "A job ID does not expire for at least 24 hours after Glacier
/// completes the job. That is, you can download the job output within the 24 hours
/// period after Amazon Glacier completes the job." Past that the job is released, which
/// is what keeps the job map (and the archive bytes its outputs hold open) from growing
/// for the life of the process.
pub const JOB_RETENTION: Duration = Duration::from_secs(24 * 60 * 60);

/// The most retrieval jobs this emulator keeps per vault before refusing to start
/// another with the modelled `PolicyEnforcedException` ("Returned if a retrieval job
/// would exceed the current data policy's retrieval rate limit").
///
/// Real Glacier bounds concurrent retrieval through its data-retrieval policy; this
/// emulator holds every job in memory, so the bound is a count. It exists so a stream of
/// tiny `InitiateJob` requests cannot grow the store without end.
pub const MAX_JOBS_PER_VAULT: usize = 100;

/// The most archives, and the most archive bytes, this emulator holds per vault.
///
/// Real Glacier has no such limit; this is the in-memory emulator's own capacity, and
/// exceeding it is answered with the modelled `ServiceUnavailableException` ("Returned
/// if the service cannot complete the request"), which is exactly what has happened.
/// The bound also caps the work an inventory render can be asked to do.
pub const MAX_ARCHIVES_PER_VAULT: usize = 1000;
/// See [`MAX_ARCHIVES_PER_VAULT`].
pub const MAX_VAULT_BYTES: i64 = 64 * 1024 * 1024;

/// One stored archive. `body` is the bytes `UploadArchive` received, byte for byte, held
/// as a [`Bytes`] so a retrieval job can take a view of it without copying the content.
#[derive(Debug, Clone)]
pub struct Archive {
    pub archive_id: String,
    pub body: Bytes,
    pub size_in_bytes: i64,
    /// The SHA-256 of each 1 MiB chunk of `body`, computed once at upload and OUTSIDE
    /// the state lock. Every tree hash this service reports for the archive or for a
    /// tree-hash-aligned range of it is folded from these leaves, so no request hashes
    /// archive bytes while it holds the store.
    pub chunk_leaves: Vec<[u8; 32]>,
    pub sha256_tree_hash: String,
    pub description: Option<String>,
    pub creation_date: String,
    /// Mint order, so an inventory lists archives oldest-first and deterministically.
    pub sequence: u64,
}

/// One row of a vault inventory, as it stood when the job that will report it was
/// initiated. Archive METADATA only: no archive bytes are copied into a job.
#[derive(Debug, Clone)]
pub struct InventoryEntry {
    pub archive_id: String,
    pub description: Option<String>,
    pub creation_date: String,
    pub size_in_bytes: i64,
    pub sha256_tree_hash: String,
}

/// One retrieval job.
///
/// An archive retrieval's `output` is a zero-copy VIEW of the archive bytes as they stood
/// when the job was initiated — the instant the retrieval is of — so a job costs its
/// metadata and not a second copy of the content. An inventory retrieval snapshots the
/// vault's archive metadata the same way and renders the document from it at download, so
/// no job holds a rendered document either.
#[derive(Debug, Clone)]
pub struct Job {
    pub job_id: String,
    pub action: String,
    pub archive_id: Option<String>,
    pub description: Option<String>,
    pub format: Option<String>,
    pub tier: Option<String>,
    pub retrieval_byte_range: Option<String>,
    pub status_code: String,
    pub status_message: Option<String>,
    pub completed: bool,
    pub creation_date: String,
    pub completion_date: Option<String>,
    pub archive_size_in_bytes: Option<i64>,
    pub sha256_tree_hash: Option<String>,
    pub archive_sha256_tree_hash: Option<String>,
    pub output: Bytes,
    /// The chunk leaves of `output` — a slice of the archive's own, so a checksum over a
    /// tree-hash-aligned part of the output costs no byte hashing either.
    pub output_leaves: Vec<[u8; 32]>,
    /// The inventory this job reports, snapshotted at initiation. Empty for an archive
    /// retrieval. At most [`MAX_ARCHIVES_PER_VAULT`] rows, and at most
    /// [`MAX_JOBS_PER_VAULT`] jobs hold one, so the product is bounded by constants.
    pub inventory: Vec<InventoryEntry>,
    /// The range-inventory options this job was initiated with, as `DescribeJob` reports
    /// them: `Format`/`StartDate`/`EndDate`/`Limit` as given, and `Marker` as the
    /// continuation token — "where to continue pagination of the vault inventory retrieval
    /// results ... If there are no more inventory items, this value is null."
    pub inventory_parameters: Option<InventoryRetrievalJobDescription>,
    /// The length of this job's rendered output, measured once when the job settles so
    /// reporting it is O(1). `None` until then.
    pub output_size_in_bytes: Option<i64>,
    pub content_type: String,
    pub sns_topic: Option<String>,
    /// The instant the clock owes this job's move to `Succeeded`. Never slept for.
    pub due_at: Instant,
    /// The instant this job is released, set when it completes. See [`JOB_RETENTION`].
    pub expires_at: Option<Instant>,
    /// Mint order, so `ListJobs` pages deterministically.
    pub sequence: u64,
}

impl Job {
    /// This job's output and the `Content-Type` it is reported with: a zero-copy view of
    /// the retrieved archive bytes, or this job's own inventory snapshot rendered into the
    /// document a caller downloads.
    pub fn output_document(&self, vault: &Vault) -> (Bytes, String) {
        if self.action == ACTION_INVENTORY_RETRIEVAL {
            let (document, content_type) =
                render_inventory(&vault.vault_arn, &self.inventory, &self.creation_date);
            (Bytes::from(document), content_type)
        } else {
            (self.output.clone(), self.content_type.clone())
        }
    }
}

/// One in-flight multipart upload. Declared now so the multipart slice
/// (`InitiateMultipartUpload`/`UploadMultipartPart`/`CompleteMultipartUpload`/`ListParts`)
/// lands without reshaping state; nothing writes it yet.
#[derive(Debug, Clone, Default)]
pub struct MultipartUpload {
    pub upload_id: String,
    pub part_size: i64,
    pub archive_description: Option<String>,
    pub creation_date: String,
    /// Range start -> (part bytes, the part's own tree hash).
    pub parts: BTreeMap<u64, (Vec<u8>, String)>,
}

/// One vault and everything stored under it.
#[derive(Debug, Default)]
pub struct Vault {
    pub vault_name: String,
    pub vault_arn: String,
    pub creation_date: String,
    pub last_inventory_date: Option<String>,
    pub archives: HashMap<String, Archive>,
    pub jobs: HashMap<String, Job>,
    pub tags: HashMap<String, String>,
    pub access_policy: Option<String>,
    pub notification_config: Option<VaultNotificationConfig>,
    pub multipart_uploads: HashMap<String, MultipartUpload>,
}

impl Vault {
    /// `DescribeVaultOutput$NumberOfArchives`, counted rather than stored.
    pub fn number_of_archives(&self) -> i64 {
        i64::try_from(self.archives.len()).unwrap_or(i64::MAX)
    }

    /// `DescribeVaultOutput$SizeInBytes`, summed rather than stored.
    pub fn size_in_bytes(&self) -> i64 {
        self.archives.values().map(|a| a.size_in_bytes).sum()
    }

    /// The vault's archives oldest-first — the order an inventory and a listing use.
    pub fn archives_in_order(&self) -> Vec<&Archive> {
        let mut archives: Vec<&Archive> = self.archives.values().collect();
        archives.sort_by_key(|a| a.sequence);
        archives
    }

    /// The vault's jobs oldest-first — the order `ListJobs` pages in.
    pub fn jobs_in_order(&self) -> Vec<&Job> {
        let mut jobs: Vec<&Job> = self.jobs.values().collect();
        jobs.sort_by_key(|j| j.sequence);
        jobs
    }

    /// A snapshot of this vault's archive metadata, oldest-first: what an inventory
    /// retrieval initiated now will report. Metadata only — no archive bytes.
    pub fn inventory_entries(&self) -> Vec<InventoryEntry> {
        self.archives_in_order()
            .into_iter()
            .map(|archive| InventoryEntry {
                archive_id: archive.archive_id.clone(),
                description: archive.description.clone(),
                creation_date: archive.creation_date.clone(),
                size_in_bytes: archive.size_in_bytes,
                sha256_tree_hash: archive.sha256_tree_hash.clone(),
            })
            .collect()
    }
}

/// Per-service state for com-amazonaws-glacier-glacier. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsGlacierGlacierState {
    /// Keyed by (accountId, vaultName).
    pub vaults: HashMap<(String, String), Vault>,
    /// Keyed by accountId. Declared for the data-retrieval-policy slice.
    pub data_retrieval_policies: HashMap<String, DataRetrievalPolicy>,
    /// Keyed by accountId. Declared for the provisioned-capacity slice.
    pub provisioned_capacity: HashMap<String, Vec<ProvisionedCapacityDescription>>,
    /// Monotonic mint counter: the source of every minted id's uniqueness.
    pub next_sequence: u64,
}

impl ComAmazonawsGlacierGlacierState {
    /// Mint a fresh opaque id of `kind` (`"archive"`, `"job"`, `"upload"`), with the
    /// sequence number that orders the object it will key.
    ///
    /// The id is opaque to a client, as Glacier's are, and unique because the counter is:
    /// the wall clock only keeps ids from repeating across emulator restarts. The caller
    /// MUST insert the object under exactly the id it gets back.
    pub fn mint_id(&mut self, kind: &str) -> (u64, String) {
        self.next_sequence += 1;
        let sequence = self.next_sequence;
        let mut hasher = Sha256::new();
        hasher.update(kind.as_bytes());
        hasher.update(b"/");
        hasher.update(sequence.to_be_bytes());
        hasher.update(unix_nanos().to_be_bytes());
        (sequence, hex::encode(hasher.finalize()))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsGlacierGlacierState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsGlacierGlacierState>)
}

/// Nanoseconds since the Unix epoch, saturating at the epoch for a clock set before it.
fn unix_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default()
}

/// Now, as the ISO-8601 (RFC-3339) string Glacier's date members carry:
/// `2026-10-21T21:08:58Z`.
///
/// Rendered through [`aws_smithy_types::DateTime`], the SDK's own instant type, so this
/// service needs no date crate of its own.
pub fn now_iso8601() -> String {
    let secs = i64::try_from(unix_nanos() / 1_000_000_000).unwrap_or(0);
    render_iso8601(::aws_smithy_types::DateTime::from_secs(secs))
}

fn render_iso8601(instant: ::aws_smithy_types::DateTime) -> String {
    instant
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// Parse an ISO-8601 (RFC-3339) instant, as `InventoryRetrievalJobInput$StartDate`
/// documents one: "a string in the ISO 8601 date format, for example
/// 2013-03-20T17:03:43Z". `None` when the text is not one, so a caller refuses it rather
/// than comparing date strings lexicographically (which orders `…:43.1Z` before `…:43Z`).
///
/// The answer is `(seconds, nanoseconds)` since the epoch, which compares in
/// chronological order.
pub fn parse_iso8601(raw: &str) -> Option<(i64, u32)> {
    let instant = ::aws_smithy_types::DateTime::from_str(
        raw.trim(),
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    Some((instant.secs(), instant.subsec_nanos()))
}

/// Resolve the `accountId` path label to the account the request acts on.
///
/// The model documents the label as "the AWS account ID ... This value must match the AWS
/// account ID associated with the credentials used to sign the request. You can either
/// specify an AWS account ID or optionally a single '-' (hyphen), in which case Amazon S3
/// Glacier uses the AWS account ID associated with the credentials used to sign the
/// request. If you specify your account ID, do not include any hyphens ('-') in the ID."
///
/// This emulator presents exactly ONE account ([`engine::emulator_account_id`]), so the
/// label resolves only when it is `-` (what the SDKs default to) or that account's own
/// id. Any other id names an account these credentials do not hold, and is refused rather
/// than quietly served from the one account there is.
pub fn resolve_account(label: &str) -> Result<String, InvalidParameterValueException> {
    let caller = ::engine::emulator_account_id();
    if label == "-" || label == caller {
        return Ok(caller.to_string());
    }
    Err(invalid_parameter_value(format!(
        "accountId {label:?} does not match the AWS account ID associated with the credentials \
         used to sign the request; specify that account ID with no hyphens, or a single '-'"
    )))
}

/// The region the request was signed for, defaulting to the emulator's own `us-east-1`
/// when the request carries no SigV4 credential scope to read one from.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// `arn:aws:glacier:<region>:<account>:vaults/<vaultName>`, built through the engine's
/// canonical ARN helper so the account segment is never a literal.
pub fn vault_arn(region: &str, vault_name: &str) -> String {
    ::engine::emulator_arn("glacier", region, &format!("vaults/{vault_name}"))
}

/// The SHA-256 of each 1 MiB chunk of `data`: the leaves of its hash tree.
///
/// Computed once per upload, before the state lock is taken, so every tree hash this
/// service later reports folds 32-byte digests rather than re-hashing archive bytes.
pub fn chunk_leaves(data: &[u8]) -> Vec<[u8; 32]> {
    let mut leaves: Vec<[u8; 32]> = data
        .chunks(usize::try_from(ONE_MEGABYTE).unwrap_or(usize::MAX))
        .map(|chunk| Sha256::digest(chunk).into())
        .collect();
    if leaves.is_empty() {
        // No chunks at all: the tree over empty content is the SHA-256 of nothing.
        leaves.push(Sha256::digest([]).into());
    }
    leaves
}

/// The leaves of a tree-hash-aligned range `[start, end]`: an aligned range covers whole
/// 1 MiB chunks, so it is exactly a contiguous run of [`chunk_leaves`].
///
/// CLAMPED to `leaf_count`, so a range this function is handed cannot index past the
/// leaves it is a range of even if a caller forgot to contain it first.
pub fn leaf_range(start: u64, end: u64, leaf_count: usize) -> std::ops::Range<usize> {
    let first = usize::try_from(start / ONE_MEGABYTE)
        .unwrap_or(usize::MAX)
        .min(leaf_count);
    let last = usize::try_from(end / ONE_MEGABYTE)
        .unwrap_or(usize::MAX)
        .saturating_add(1)
        .clamp(first, leaf_count);
    first..last
}

/// The SHA-256 tree hash of content whose leaves are already computed, lowercase hex.
///
/// Glacier's checksum: SHA-256 each 1 MiB chunk of the content ([`chunk_leaves`]), then
/// hash the concatenation of each adjacent pair of digests, promoting an odd digest
/// unchanged, until one digest remains. Content shorter than a chunk hashes to its plain
/// SHA-256. This is the ONE entry point to the checksum: a caller folds leaves, so no
/// request re-hashes stored bytes.
pub fn tree_hash_from_leaves(leaves: &[[u8; 32]]) -> String {
    hex::encode(fold_leaves(leaves))
}

fn fold_leaves(leaves: &[[u8; 32]]) -> [u8; 32] {
    let mut level: Vec<[u8; 32]> = leaves.to_vec();
    if level.is_empty() {
        level.push(Sha256::digest([]).into());
    }
    while level.len() > 1 {
        let mut parents: Vec<[u8; 32]> = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            match pair {
                [left, right] => {
                    let mut hasher = Sha256::new();
                    hasher.update(left);
                    hasher.update(right);
                    parents.push(hasher.finalize().into());
                }
                // An odd digest has no sibling at this level: promote it unchanged.
                [lone] => parents.push(*lone),
                _ => {}
            }
        }
        level = parents;
    }
    level[0]
}

/// Is `[start, end]` a tree-hash-aligned range of `total` bytes?
///
/// `JobParameters$RetrievalByteRange`: "StartByteValue must be divisible by 1 MB and
/// EndByteValue plus 1 must be divisible by 1 MB or be the end of the archive specified
/// as the archive byte size value minus 1."
///
/// CONTAINMENT IS PART OF THE PREDICATE: a range that runs past `total` is not a range of
/// `total` bytes, however well it divides. `end` reaches this from a request string, so
/// `end + 1` is computed with [`u64::checked_add`] too: an `end` of `u64::MAX` is not an
/// aligned range, it is a refusal.
pub fn is_megabyte_aligned(start: u64, end: u64, total: u64) -> bool {
    let Some(next) = end.checked_add(1) else {
        return false;
    };
    start <= end
        && end < total
        && start.is_multiple_of(ONE_MEGABYTE)
        && (next.is_multiple_of(ONE_MEGABYTE) || next == total)
}

/// The length of the inclusive range `[start, end]`, or `None` when it is not one.
///
/// Both bounds come from a request string, so the subtraction and the `+ 1` are checked:
/// an inverted or saturated range is a refusal, never a wrapped length.
pub fn inclusive_range_len(start: u64, end: u64) -> Option<u64> {
    end.checked_sub(start)?.checked_add(1)
}

/// Parse a `JobParameters$RetrievalByteRange`, "StartByteValue-EndByteValue".
///
/// Refuses an inverted range, and a bound at `u64::MAX` which no content can hold, so no
/// caller of this parser can observe a wrapped value.
pub fn parse_retrieval_byte_range(raw: &str) -> Option<(u64, u64)> {
    let (start, end) = raw.trim().split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end: u64 = end.trim().parse().ok()?;
    (start <= end && end < u64::MAX).then_some((start, end))
}

/// Parse an HTTP `Range` request header over `total` bytes: `bytes=START-END`, or
/// `bytes=START-` for "to the end". `None` when the header is not a single satisfiable
/// byte range of this content.
pub fn parse_http_range(raw: &str, total: u64) -> Option<(u64, u64)> {
    let spec = raw.trim().strip_prefix("bytes=")?.trim();
    let (start, end) = spec.split_once('-')?;
    let start: u64 = start.trim().parse().ok()?;
    let end = match end.trim() {
        "" => total.checked_sub(1)?,
        value => value.parse().ok()?,
    };
    let end = end.min(total.checked_sub(1)?);
    (start <= end).then_some((start, end))
}

/// Settle every job of `vault` whose completion the clock already owes, and release every
/// one whose retention the clock has already run out.
///
/// These are the only writes a reporting operation (`DescribeJob`, `ListJobs`,
/// `GetJobOutput`) makes, and both were owed at an instant that has already passed
/// whether or not anybody called: the move to `Succeeded` at [`JOB_COMPLETION_DELAY`]
/// after `InitiateJob`, and the expiry of a completed job at [`JOB_RETENTION`] after it
/// completed. Settling records that the output is ready; expiry drops the job and the
/// view it holds of the archive bytes.
pub fn settle_due_jobs(vault: &mut Vault) {
    let now = Instant::now();
    vault
        .jobs
        .retain(|_, job| job.expires_at.is_none_or(|expiry| now < expiry));
    if !vault.jobs.values().any(|j| !j.completed && now >= j.due_at) {
        return;
    }
    let settled_at = now_iso8601();
    let vault_arn = vault.vault_arn.clone();
    let mut inventory_settled = false;
    for job in vault.jobs.values_mut() {
        if job.completed || now < job.due_at {
            continue;
        }
        job.expires_at = Some(now + JOB_RETENTION);
        // Measured once, here, from this job's own snapshot: `DescribeJob`/`ListJobs`
        // then report the size without rendering the document again.
        job.output_size_in_bytes = Some(if job.action == ACTION_INVENTORY_RETRIEVAL {
            let (document, _) = render_inventory(&vault_arn, &job.inventory, &job.creation_date);
            i64::try_from(document.len()).unwrap_or(i64::MAX)
        } else {
            i64::try_from(job.output.len()).unwrap_or(i64::MAX)
        });
        job.completed = true;
        job.status_code = STATUS_SUCCEEDED.to_string();
        job.status_message = Some(STATUS_SUCCEEDED.to_string());
        job.completion_date = Some(settled_at.clone());
        if job.action == ACTION_INVENTORY_RETRIEVAL {
            inventory_settled = true;
        }
    }
    if inventory_settled {
        vault.last_inventory_date = Some(settled_at);
    }
}

/// Glacier's error bodies carry `type`/`code` beside `message`; every error in this
/// service's model is one of these three shapes with that same trio. `type` is the
/// error's `errorKind` as the wire spells it.
const CLIENT_ERROR_TYPE: &str = "Client";

/// `ResourceNotFoundException` (404) — "Returned if the specified resource (such as a
/// vault, upload ID, or job ID) doesn't exist."
pub fn resource_not_found(message: impl Into<String>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        r#type: Some(CLIENT_ERROR_TYPE.to_string()),
        code: Some("ResourceNotFoundException".to_string()),
        message: Some(message.into()),
    }
}

/// `InvalidParameterValueException` (400) — "Returned if a parameter of the request is
/// incorrectly specified."
pub fn invalid_parameter_value(message: impl Into<String>) -> InvalidParameterValueException {
    InvalidParameterValueException {
        r#type: Some(CLIENT_ERROR_TYPE.to_string()),
        code: Some("InvalidParameterValueException".to_string()),
        message: Some(message.into()),
    }
}

/// `MissingParameterValueException` (400) — "Returned if a required header or parameter
/// is missing from the request."
pub fn missing_parameter_value(message: impl Into<String>) -> MissingParameterValueException {
    MissingParameterValueException {
        r#type: Some(CLIENT_ERROR_TYPE.to_string()),
        code: Some("MissingParameterValueException".to_string()),
        message: Some(message.into()),
    }
}

/// `LimitExceededException` (400) — "Returned if the request results in a vault or
/// account limit being exceeded."
pub fn limit_exceeded(message: impl Into<String>) -> LimitExceededException {
    LimitExceededException {
        r#type: Some(CLIENT_ERROR_TYPE.to_string()),
        code: Some("LimitExceededException".to_string()),
        message: Some(message.into()),
    }
}

/// `ServiceUnavailableException` (500) — "Returned if the service cannot complete the
/// request." This service answers it when it is mounted on an engine that holds no store,
/// and when a vault has reached the in-memory capacity this emulator can hold
/// ([`MAX_ARCHIVES_PER_VAULT`] / [`MAX_VAULT_BYTES`]) — in both cases the service really
/// cannot complete the request, and nothing about the request itself is wrong.
pub fn service_unavailable(message: impl Into<String>) -> ServiceUnavailableException {
    ServiceUnavailableException {
        r#type: Some("Server".to_string()),
        code: Some("ServiceUnavailableException".to_string()),
        message: Some(message.into()),
    }
}

/// `PolicyEnforcedException` (400) — "Returned if a retrieval job would exceed the
/// current data policy's retrieval rate limit."
pub fn policy_enforced(message: impl Into<String>) -> PolicyEnforcedException {
    PolicyEnforcedException {
        r#type: Some(CLIENT_ERROR_TYPE.to_string()),
        code: Some("PolicyEnforcedException".to_string()),
        message: Some(message.into()),
    }
}

/// The message [`service_unavailable`] carries when the engine holds no store.
pub const NO_STORE_MESSAGE: &str =
    "the glacier emulator is mounted on an engine that holds no state";

/// The refusal every operation that addresses a vault by name shares: the vault is not
/// there. The message names the ARN the request resolved to, so a caller can see which
/// account and region it actually addressed.
pub fn vault_not_found(region: &str, vault_name: &str) -> ResourceNotFoundException {
    resource_not_found(format!(
        "Vault not found for ARN: {}",
        vault_arn(region, vault_name)
    ))
}

/// Check a vault name against the rule `CreateVault` documents: "Names can be between 1
/// and 255 characters long. Allowed characters are a-z, A-Z, 0-9, '_' (underscore), '-'
/// (hyphen), and '.' (period)." Returns the refusal message when it does not hold.
pub fn vault_name_violation(vault_name: &str) -> Option<String> {
    let length = vault_name.chars().count();
    if !(MIN_VAULT_NAME_LEN..=MAX_VAULT_NAME_LEN).contains(&length) {
        return Some(format!(
            "Vault name must be between {MIN_VAULT_NAME_LEN} and {MAX_VAULT_NAME_LEN} characters long, but was {length}"
        ));
    }
    if let Some(bad) = vault_name
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')))
    {
        return Some(format!(
            "Vault name contains the disallowed character {bad:?}; allowed characters are a-z, A-Z, 0-9, '_', '-' and '.'"
        ));
    }
    None
}

/// `DescribeVaultOutput` for `vault` — what `DescribeVault` returns and what each entry
/// of `ListVaults`' `VaultList` is.
pub fn vault_description(vault: &Vault) -> DescribeVaultOutput {
    DescribeVaultOutput {
        vault_arn: Some(vault.vault_arn.clone()),
        vault_name: Some(vault.vault_name.clone()),
        creation_date: Some(vault.creation_date.clone()),
        last_inventory_date: vault.last_inventory_date.clone(),
        number_of_archives: vault.number_of_archives(),
        size_in_bytes: vault.size_in_bytes(),
    }
}

/// `GlacierJobDescription` for `job` — what `DescribeJob` returns and what each entry of
/// `ListJobs`' `JobList` is.
pub fn job_description(job: &Job, vault: &Vault) -> DescribeJobOutput {
    let inventory = job.action == ACTION_INVENTORY_RETRIEVAL;
    DescribeJobOutput {
        job_id: Some(job.job_id.clone()),
        job_description: job.description.clone(),
        action: Some(job.action.clone()),
        archive_id: job.archive_id.clone(),
        vault_arn: Some(vault.vault_arn.clone()),
        creation_date: Some(job.creation_date.clone()),
        completed: job.completed,
        status_code: Some(job.status_code.clone()),
        status_message: job.status_message.clone(),
        archive_size_in_bytes: job.archive_size_in_bytes,
        // Only an inventory retrieval has an inventory size, and only once its output is
        // the thing a caller can download; the size was measured when the job settled.
        inventory_size_in_bytes: inventory.then_some(job.output_size_in_bytes).flatten(),
        sns_topic: job.sns_topic.clone(),
        completion_date: job.completion_date.clone(),
        sha256_tree_hash: job.sha256_tree_hash.clone(),
        archive_sha256_tree_hash: job.archive_sha256_tree_hash.clone(),
        retrieval_byte_range: job.retrieval_byte_range.clone(),
        tier: job.tier.clone(),
        inventory_retrieval_parameters: job.inventory_parameters.clone(),
        job_output_path: None,
        select_parameters: None,
        output_location: None,
    }
}

/// Render an inventory snapshot as the JSON document `GetJobOutput` hands back, with the
/// `Content-Type` it is reported under.
///
/// Only the `JSON` format the model documents as the default is rendered here; a job
/// asking for `CSV` is refused at `InitiateJob` with the scaffold's 501 rather than
/// answered with a document in the wrong format.
pub fn render_inventory(
    vault_arn: &str,
    archives: &[InventoryEntry],
    inventory_date: &str,
) -> (Vec<u8>, String) {
    let body = serde_json::json!({
        "VaultARN": vault_arn,
        "InventoryDate": inventory_date,
        "ArchiveList": archives
            .iter()
            .map(|archive| serde_json::json!({
                "ArchiveId": archive.archive_id,
                "ArchiveDescription": archive.description.clone().unwrap_or_default(),
                "CreationDate": archive.creation_date,
                "Size": archive.size_in_bytes,
                "SHA256TreeHash": archive.sha256_tree_hash,
            }))
            .collect::<Vec<_>>(),
    });
    let bytes = serde_json::to_vec(&body).unwrap_or_default();
    (bytes, "application/json".to_string())
}
