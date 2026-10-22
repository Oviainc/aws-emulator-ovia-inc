// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-backupsearch-cryobackupsearchservice service.
//!
//! The service's primary resource is the SEARCH JOB: `StartSearchJob` mints one,
//! `GetSearchJob`/`ListSearchJobs`/`ListSearchJobResults` report it, `StopSearchJob`
//! ends it, and `TagResource`/`ListTagsForResource` reach its tags through its ARN.
//! Every one of those answers is computed from the record held here.
//!
//! ## The clock, not the reader, moves a status
//!
//! A search job AWS returns is `RUNNING` — a non-terminal status — so the service owes
//! it a later status without anybody calling again. That owed transition is stored as a
//! DUE INSTANT ([`SearchJob::next_status_due`]) and materialised by
//! [`ComAmazonawsBackupsearchCryobackupsearchserviceState::settle`], which every handler
//! calls first: a read therefore reports what the clock has already made due and never
//! advances a job because it was polled.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    CurrentSearchProgress, ItemFilters, ResultItem, SearchScope, SearchScopeSummary,
};

/// The `SearchJobState` values the model declares, so no handler spells one as a bare
/// literal. The full set is `ops::types::SEARCH_JOB_STATE_VALUES`.
pub const RUNNING: &str = "RUNNING";
/// See [`RUNNING`].
pub const COMPLETED: &str = "COMPLETED";
/// See [`RUNNING`].
pub const STOPPING: &str = "STOPPING";
/// See [`RUNNING`].
pub const STOPPED: &str = "STOPPED";
/// See [`RUNNING`].
pub const FAILED: &str = "FAILED";

/// The statuses a search job never leaves: a scan that ended, a stop that took effect,
/// or a scan that failed. `RUNNING` and `STOPPING` are the two transient ones.
pub const TERMINAL_STATES: &[&str] = &[COMPLETED, STOPPED, FAILED];

/// How long after the call that starts a transition the clock owes the next status
/// (`RUNNING` -> `COMPLETED` after `StartSearchJob`, `STOPPING` -> `STOPPED` after
/// `StopSearchJob`).
///
/// AWS documents no duration for either — how long a scan takes depends on the backups
/// in scope — so this span is CHOSEN, not modelled: 500 ms, inside the project's
/// 250-750 ms band. It is deliberately non-zero, so a caller acting straight after
/// `StartSearchJob` still observes `RUNNING` and `StopSearchJob` can still accept it.
/// Nothing sleeps for it: the due instant is stored and compared against the clock.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// `MaxResults`'s modelled `@default(1000)`, shared by `ListSearchJobs` and
/// `ListSearchJobResults`: the page size to serve when the member is absent.
pub const DEFAULT_MAX_RESULTS: usize = 1000;

/// The upper bound of `MaxResults`'s modelled `@range(min: 1, max: 1000)`. Kept apart
/// from [`DEFAULT_MAX_RESULTS`] because the model states them separately and either one
/// can move without the other; a page is clamped to THIS, so a response cannot be
/// amplified past what the model allows even if a request slips the range check.
pub const MAX_PAGE_SIZE: usize = 1000;

/// The ceiling on how many search jobs this store holds.
///
/// A generated emulator keeps its resources in memory for the life of the process, and
/// `StartSearchJob` is the only thing that adds one, so the store needs a maximum entry
/// count: neither a TTL nor an eviction policy is available to it, because `GetSearchJob`
/// must still answer what `StartSearchJob` created. The operation that grows the store
/// refuses past this cap with the service's own modelled `ServiceQuotaExceededException`
/// (HTTP 402), which the model declares on exactly the two `Start` operations and on no
/// read. The number is the emulator's own bound, not a published AWS default — AWS
/// publishes the search-job quota as adjustable, so no single value is the contract —
/// and it is set high enough that no ordinary workflow meets it.
pub const MAX_SEARCH_JOBS: usize = 5_000;

/// The ceiling on how many tags one search job carries, with the caps on one tag's key
/// and value. The model puts no length or count constraint on `TagMap`, so these are the
/// emulator's own bound on caller-driven growth of a stored record, set at the limits AWS
/// applies to resource tags generally (50 tags, a 128-character key, a 256-character
/// value). `StartSearchJob` and `TagResource` refuse a tag set past them with a
/// `ValidationException` naming the member and the bound.
pub const MAX_TAGS_PER_JOB: usize = 50;

/// See [`MAX_TAGS_PER_JOB`].
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// See [`MAX_TAGS_PER_JOB`].
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The ARN resource segment of a search job: `search-job/<SearchJobIdentifier>`.
pub const SEARCH_JOB_RESOURCE: &str = "search-job";

/// The one wording of the server fault every handler answers with when this emulator's
/// search-job store cannot be reached, so the text cannot drift between operations.
pub const STORE_UNAVAILABLE: &str = "the search job store is unavailable";

/// The Region an unsigned request is attributed to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The AWS Region a request was signed for, which is the Region the resources it creates
/// live in. Falls back to the emulator's default Region for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// A `SearchJobArn`: `arn:aws:backup-search:<region>:<account>:search-job/<id>`.
pub fn search_job_arn(signing_name: &str, region: &str, identifier: &str) -> String {
    ::engine::emulator_arn(
        signing_name,
        region,
        &format!("{SEARCH_JOB_RESOURCE}/{identifier}"),
    )
}

/// One persisted search job. Every member a modelled response carries is read from
/// here; nothing in a response is recomputed from the request.
#[derive(Debug, Clone)]
pub struct SearchJob {
    pub identifier: String,
    pub arn: String,
    pub name: Option<String>,
    /// One of [`TERMINAL_STATES`], [`RUNNING`] or [`STOPPING`].
    pub status: String,
    pub creation_time: DateTime,
    pub completion_time: Option<DateTime>,
    pub status_message: Option<String>,
    pub encryption_key_arn: Option<String>,
    pub search_scope: SearchScope,
    pub item_filters: ItemFilters,
    pub search_scope_summary: SearchScopeSummary,
    pub current_search_progress: CurrentSearchProgress,
    /// The matches the scan recorded. `ListSearchJobResults` pages over exactly this.
    pub results: Vec<ResultItem>,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the call that minted this job carried, if any. Idempotency is
    /// keyed off the job records themselves, so a caller cannot grow a second index by
    /// varying only the token.
    pub client_token: Option<String>,
    /// Canonical form of the `StartSearchJob` body this job was created from. A repeat
    /// of the same body under the same `ClientToken` is idempotent; a different body
    /// under that token is a conflict.
    pub request_fingerprint: String,
    /// The status the clock owes this job, and the instant it is owed from. `None` for
    /// a job in a terminal status, which owes nothing.
    pub next_status_due: Option<(SystemTime, &'static str)>,
}

impl SearchJob {
    /// Whether this job has reached a status it never leaves.
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATES.contains(&self.status.as_str())
    }

    /// Apply the owed status transition if the clock has already made it due.
    ///
    /// The scan's progress is brought to the scope it was given — a scan that ran to
    /// `COMPLETED` has scanned everything in scope and matched exactly the results it
    /// recorded; one that was `STOPPED` keeps the progress it had reached.
    fn settle(&mut self, now: SystemTime) {
        let Some((due, next)) = self.next_status_due else {
            return;
        };
        if now < due {
            return;
        }
        self.status = next.to_string();
        self.next_status_due = None;
        self.completion_time = Some(timestamp(due));
        if next == COMPLETED {
            self.current_search_progress = CurrentSearchProgress {
                recovery_points_scanned_count: self
                    .search_scope_summary
                    .total_recovery_points_to_scan_count,
                items_scanned_count: self.search_scope_summary.total_items_to_scan_count,
                items_matched_count: Some(self.results.len() as i64),
            };
        }
    }
}

/// Per-service state for com-amazonaws-backupsearch-cryobackupsearchservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBackupsearchCryobackupsearchserviceState {
    /// Every search job, keyed by its `SearchJobIdentifier`.
    pub search_jobs: HashMap<String, SearchJob>,
    /// Identifiers in creation order, so `ListSearchJobs` pages deterministically.
    pub job_order: Vec<String>,
    /// How many identifiers this store has minted; the next one is the one after.
    minted: u64,
}

impl ComAmazonawsBackupsearchCryobackupsearchserviceState {
    /// Materialise every status transition the clock has already made due. Call this
    /// FIRST in every handler, including a read: it only applies a change that was
    /// owed at `now` whether or not anyone called.
    pub fn settle(&mut self, now: SystemTime) {
        for job in self.search_jobs.values_mut() {
            job.settle(now);
        }
    }

    /// The job `identifier` names, or `None` when this account holds no such job.
    pub fn job(&self, identifier: &str) -> Option<&SearchJob> {
        self.search_jobs.get(identifier)
    }

    /// The job `identifier` names, mutably.
    pub fn job_mut(&mut self, identifier: &str) -> Option<&mut SearchJob> {
        self.search_jobs.get_mut(identifier)
    }

    /// The job whose minted ARN is `arn` — how the tagging operations, which take a
    /// `ResourceArn`, resolve the resource they name.
    pub fn job_by_arn(&self, arn: &str) -> Option<&SearchJob> {
        self.search_jobs.values().find(|job| job.arn == arn)
    }

    /// The job whose minted ARN is `arn`, mutably.
    pub fn job_by_arn_mut(&mut self, arn: &str) -> Option<&mut SearchJob> {
        let identifier = self.job_by_arn(arn).map(|job| job.identifier.clone())?;
        self.search_jobs.get_mut(&identifier)
    }

    /// Every job in creation order. `job_order` and `search_jobs` are written together,
    /// so an identifier in the order always resolves.
    pub fn jobs_in_order(&self) -> impl Iterator<Item = &SearchJob> {
        self.job_order
            .iter()
            .filter_map(|identifier| self.search_jobs.get(identifier))
    }

    /// The job the `StartSearchJob` call carrying `token` minted, if that call was
    /// made. Read off the job records, so the idempotency index cannot outgrow
    /// [`MAX_SEARCH_JOBS`].
    pub fn job_by_client_token(&self, token: &str) -> Option<&SearchJob> {
        self.search_jobs
            .values()
            .find(|job| job.client_token.as_deref() == Some(token))
    }

    /// Whether this store is already holding [`MAX_SEARCH_JOBS`] jobs, which is when
    /// `StartSearchJob` must refuse rather than mint another.
    pub fn is_full(&self) -> bool {
        self.search_jobs.len() >= MAX_SEARCH_JOBS
    }

    /// Store a newly minted job, keeping the creation order.
    pub fn insert_job(&mut self, job: SearchJob) {
        self.job_order.push(job.identifier.clone());
        self.search_jobs.insert(job.identifier.clone(), job);
    }

    /// A fresh `SearchJobIdentifier` no job in this store holds.
    ///
    /// `GenericId` carries no pattern or length in the model, so what the member
    /// promises is an opaque string that identifies one job: this one counts the jobs
    /// the store has minted, so it is unique, monotonic in creation order and stable
    /// across reads, and no part of it is derived from the request. The caller STORES
    /// what this returns — nothing recomputes it.
    pub fn mint_identifier(&mut self) -> String {
        self.minted = self.minted.saturating_add(1);
        format!("searchjob-{:012}", self.minted)
    }
}

/// A timestamp for `instant`, truncated to milliseconds — the precision the model's
/// own documentation gives these members ("accurate to milliseconds").
pub fn timestamp(instant: SystemTime) -> DateTime {
    let millis = instant
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX));
    DateTime::from_millis(millis)
}

/// A `NextToken` for the page that resumes at `offset` of the sequence `context` names.
///
/// The context is part of the token because an offset alone says nothing about WHICH
/// sequence it indexes: a token issued for one `ByStatus` filter (or one search job's
/// results) would otherwise silently resume a different sequence at that position and
/// answer 200 with a page that skips or repeats entries.
pub fn encode_next_token(context: &str, offset: usize) -> String {
    format!("{offset}@{context}")
}

/// Where a `NextToken` resumes, as an offset into the sequence `context` names.
///
/// `Err` is the caller's problem: a token this service did not issue, one issued for a
/// different sequence, or one that points past the end of what it is paging over.
pub fn decode_next_token(
    token: Option<&str>,
    context: &str,
    len: usize,
) -> Result<usize, String> {
    let Some(token) = token else {
        return Ok(0);
    };
    let malformed =
        || format!("NextToken '{token}' is not a pagination token this service issued");
    let (offset, token_context) = token.split_once('@').ok_or_else(malformed)?;
    if token_context != context {
        return Err(format!(
            "NextToken '{token}' was issued for a different request and cannot resume this one"
        ));
    }
    let offset: usize = offset.parse().map_err(|_| malformed())?;
    if offset > len {
        return Err(format!(
            "NextToken '{token}' resumes past the end of the results"
        ));
    }
    Ok(offset)
}

/// One page of `items` starting at `offset`, with the token that resumes after it
/// (`None` on the last page, which is how a paginating client knows to stop). The token
/// carries `context`, the sequence it indexes.
pub fn page<T: Clone>(
    items: &[T],
    offset: usize,
    max_results: usize,
    context: &str,
) -> (Vec<T>, Option<String>) {
    let end = offset.saturating_add(max_results).min(items.len());
    let page = items[offset.min(items.len())..end].to_vec();
    let next = (end < items.len()).then(|| encode_next_token(context, end));
    (page, next)
}

/// The sequence a `ListSearchJobs` page indexes: the jobs in this status, or every job
/// when the request carries no `ByStatus`. Part of the page's `NextToken`, so a token
/// cannot resume a differently filtered listing.
pub fn jobs_page_context(by_status: Option<&str>) -> String {
    format!("jobs:{}", by_status.unwrap_or("ALL"))
}

/// The sequence a `ListSearchJobResults` page indexes: one search job's results.
pub fn results_page_context(identifier: &str) -> String {
    format!("results:{identifier}")
}

/// The page size to serve: the requested `MaxResults` clamped to the modelled maximum,
/// or the modelled default when the member is absent (the framework leaves an absent
/// integer at zero).
pub fn page_size(max_results: i32) -> usize {
    match usize::try_from(max_results) {
        Ok(requested) if requested > 0 => requested.min(MAX_PAGE_SIZE),
        _ => DEFAULT_MAX_RESULTS,
    }
}

/// Refuse one tag entry whose key or value is longer than this store's cap, naming the
/// member and the bound the way a validation refusal does.
pub fn check_tag_entry(key: &str, value: &str) -> Result<(), String> {
    let key_length = key.chars().count();
    if key_length > MAX_TAG_KEY_LENGTH {
        return Err(format!(
            "Tags key is {key_length} characters, more than the maximum of {MAX_TAG_KEY_LENGTH}"
        ));
    }
    let value_length = value.chars().count();
    if value_length > MAX_TAG_VALUE_LENGTH {
        return Err(format!(
            "Tags value for key '{key}' is {value_length} characters, more than the maximum of {MAX_TAG_VALUE_LENGTH}"
        ));
    }
    Ok(())
}

/// Refuse a tag set that would leave one job holding more than [`MAX_TAGS_PER_JOB`]
/// tags. `count` is the number of tags the job would hold after the write.
pub fn check_tag_count(count: usize) -> Result<(), String> {
    if count > MAX_TAGS_PER_JOB {
        return Err(format!(
            "Tags would hold {count} entries, more than the maximum of {MAX_TAGS_PER_JOB} for one resource"
        ));
    }
    Ok(())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBackupsearchCryobackupsearchserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBackupsearchCryobackupsearchserviceState>)
}
