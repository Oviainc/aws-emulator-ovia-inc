// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-backupsearch-cryobackupsearchservice service.
//!
//! The service's primary resource is the SEARCH JOB: `StartSearchJob` mints one,
//! `GetSearchJob`/`ListSearchJobs`/`ListSearchJobResults`/`ListSearchJobBackups` report
//! it, `StopSearchJob` ends it, and `TagResource`/`ListTagsForResource` reach its tags
//! through its ARN.
//!
//! Its second resource is the EXPORT JOB ([`ExportJob`]): `StartSearchResultExportJob`
//! mints one for a search job whose results have settled, and
//! `GetSearchResultExportJob`/`ListSearchResultExportJobs` report exactly that record.
//! Every one of those answers is computed from the records held here.
//!
//! BOTH are taggable resources of this service in their own right — each is minted by an
//! operation taking a `TagMap` and each ARN is an `@arnReference` to this service — so
//! the tag APIs resolve their `ResourceArn` against every record held here
//! ([`ComAmazonawsBackupsearchCryobackupsearchserviceState::tags_by_arn`]), not against
//! one resource type.
//!
//! ## The clock, not the reader, moves a status
//!
//! A search job AWS returns is `CREATING` — a non-terminal status — so the service owes
//! it a later status without anybody calling again, and then another: a scan is set up
//! (`CREATING`), runs (`RUNNING`) and ends (a terminal status). Those owed transitions
//! are stored as DUE INSTANTS ([`SearchJob::owed_statuses`]) and materialised by
//! [`ComAmazonawsBackupsearchCryobackupsearchserviceState::settle`], which every handler
//! calls first: a read therefore reports what the clock has already made due and never
//! advances a job because it was polled.
//!
//! The clock drives one more thing a caller can observe: a `ClientToken`'s validity
//! window ([`CLIENT_TOKEN_VALIDITY`], [`ClientToken`]). A token takes part in idempotency
//! only while that window is open, so the same `settle` that materialises a due status
//! drops a token the clock has carried past it — and the next call under that token is a
//! new request rather than a repeat or a conflict.
//!
//! WHICH status that is follows from the job's resolved scope, not from the operation
//! that started it: [`scan_outcome`] turns the recovery points a scope resolved to
//! ([`ComAmazonawsBackupsearchCryobackupsearchserviceState::recovery_points_in_scope`])
//! into the terminal status the scan is owed, and the `StatusMessage` that status comes
//! with travels with it.
//!
//! ## Two layers: backups, and the items inside them
//!
//! A scan resolves a `SearchScope` to recovery points and then gets through the ITEMS
//! their backup indexes hold ([`IndexedRecoveryPoint`]) — the objects of a backed-up
//! bucket, the files of a backed-up volume. That inner layer is what a job's `ItemFilters`
//! select from ([`item_matches_filters`]), what `TotalItemsToScanCount` and
//! `ItemsScannedCount` count ([`scanned_items`]) and what `ListSearchJobResults` pages, so
//! every one of those answers is counted from the records this store holds rather than
//! assumed by a handler.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_types::DateTime;

use crate::ops::types::{
    CurrentSearchProgress, EBSItemFilter, EBSResultItem, ExportSpecification, ItemFilters,
    LongCondition, ResultItem, S3ItemFilter, S3ResultItem, SearchJobBackupsResult, SearchScope,
    SearchScopeSummary, StringCondition, TimeCondition,
};

/// The HTTP status a successful call of this service that has NO content to report
/// answers with.
///
/// The three tag operations all leave their success code DEFAULTED in the model (`@http`
/// carries no `code:`, i.e. `smithy_query(query="operation", operation="TagResource")`
/// reports `code: 200, codeDefaulted: true`), so the Smithy default of 200 is not a
/// statement about this service: the status is a behavioural fact and the recorded AWS
/// responses are what state it. Of those three, the two whose output is a declared
/// structure with NO members — `TagResourceResponse` and `UntagResourceResponse` — have
/// no content to parse and are answered with 204 No Content. A 200 there promises a body
/// the response does not carry, so every client path gated on 204 (conditional body
/// parsing, caches and proxies that key on it) takes the wrong branch.
///
/// The third, `ListTagsForResource`, keeps the defaulted 200: its output declares `Tags`,
/// so it HAS content to report. Nor does this extend to `StopSearchJob`, whose output
/// structure is just as empty but whose model carries an EXPLICIT `@http(code: 200)` (no
/// `codeDefaulted`): there the model states the status, so it keeps 200.
pub const NO_CONTENT_SUCCESS_STATUS: u16 = 204;

/// The `SearchJobState` values the model declares, so no handler spells one as a bare
/// literal. The full set is `ops::types::SEARCH_JOB_STATE_VALUES`.
pub const RUNNING: &str = "RUNNING";
/// The status a search job the service has accepted but not yet started scanning for is
/// in — the FIRST status any caller can observe after `StartSearchJob`, before
/// [`RUNNING`].
///
/// This value is NOT in the model's `SearchJobState` enum
/// (`ops::types::SEARCH_JOB_STATE_VALUES`), and that is not a contradiction: restJson1
/// enums are open on the wire, and the service's own recordings report a just-started
/// search job with this status — the status-parameterised refusal an export started
/// back-to-back with `StartSearchJob` is answered with names it. A client reading the
/// status as a string therefore sees the setting-up phase it sees on AWS; one that
/// matches only the closed enum treats it as an unknown variant, exactly as it would
/// against AWS.
///
/// It is a TRANSIENT status (it is not in [`TERMINAL_STATES`]) and the clock owes the job
/// [`RUNNING`] after [`CREATING_DELAY`], so no caller is left in it.
pub const CREATING: &str = "CREATING";
/// See [`RUNNING`].
pub const COMPLETED: &str = "COMPLETED";
/// See [`RUNNING`].
pub const STOPPING: &str = "STOPPING";
/// See [`RUNNING`].
pub const STOPPED: &str = "STOPPED";
/// See [`RUNNING`].
pub const FAILED: &str = "FAILED";

/// The statuses a search job never leaves: a scan that ended, a stop that took effect,
/// or a scan that failed. `CREATING`, `RUNNING` and `STOPPING` are the transient ones.
pub const TERMINAL_STATES: &[&str] = &[COMPLETED, STOPPED, FAILED];

/// The comparison an item-filter condition states, as the model's three condition-operator
/// enums declare them (`types::STRING_CONDITION_OPERATOR_VALUES`,
/// `types::LONG_CONDITION_OPERATOR_VALUES`, `types::TIME_CONDITION_OPERATOR_VALUES`), so
/// no comparison spells one as a bare literal. This one and [`NOT_EQUALS_TO`] are declared
/// by all three; the orderings by the numeric and time enums, and the substring ones by the
/// string enum. `EQUALS_TO` is also the `@default` every `Operator` member carries.
pub const EQUALS_TO: &str = "EQUALS_TO";
/// See [`EQUALS_TO`].
pub const NOT_EQUALS_TO: &str = "NOT_EQUALS_TO";
/// See [`EQUALS_TO`].
pub const LESS_THAN_EQUAL_TO: &str = "LESS_THAN_EQUAL_TO";
/// See [`EQUALS_TO`].
pub const GREATER_THAN_EQUAL_TO: &str = "GREATER_THAN_EQUAL_TO";
/// See [`EQUALS_TO`].
pub const CONTAINS: &str = "CONTAINS";
/// See [`EQUALS_TO`].
pub const DOES_NOT_CONTAIN: &str = "DOES_NOT_CONTAIN";
/// See [`EQUALS_TO`].
pub const BEGINS_WITH: &str = "BEGINS_WITH";
/// See [`EQUALS_TO`].
pub const DOES_NOT_BEGIN_WITH: &str = "DOES_NOT_BEGIN_WITH";
/// See [`EQUALS_TO`].
pub const ENDS_WITH: &str = "ENDS_WITH";
/// See [`EQUALS_TO`].
pub const DOES_NOT_END_WITH: &str = "DOES_NOT_END_WITH";

/// The statuses `StopSearchJob` is already satisfied by.
///
/// The operation is `@idempotent` and asks for ONE end state: the job stopped. A job
/// that is `STOPPED` is in that state and one that is `STOPPING` is on its way to it, so
/// a repeat has nothing left to do and answers the empty 200 the model declares. Only a
/// terminal status this operation does NOT produce ([`COMPLETED`], [`FAILED`]) is a
/// genuine conflict, because the job ended in a state the caller's request cannot reach.
pub const STOP_SATISFIED_STATES: &[&str] = &[STOPPING, STOPPED];

/// The search-job statuses an export may be started from.
///
/// An export transmits the RESULTS of a search job, so the scan has to have finished
/// producing them: a search job still scanning (`RUNNING`) or still winding down
/// (`STOPPING`) has no settled result set to export, and `FAILED` is terminal but
/// produced none. The service's own recordings confirm this set — an export started for
/// a search job that ended `FAILED` is refused — and a start from any other status is
/// refused rather than accepted into a success nothing could back.
pub const EXPORTABLE_SEARCH_JOB_STATES: &[&str] = &[COMPLETED, STOPPED];

/// How long after the call that starts a transition the clock owes the next status (the
/// terminal status [`scan_outcome`] gives a new search job, `STOPPING` -> `STOPPED` after
/// `StopSearchJob`, and an export job's `RUNNING` -> `COMPLETED` after
/// `StartSearchResultExportJob`).
///
/// AWS documents no duration for any of them — how long a scan or an export takes
/// depends on the backups in scope — so this span is CHOSEN, not modelled: 500 ms,
/// inside the project's 250-750 ms band. It is deliberately non-zero, so a caller acting
/// straight after `StartSearchJob` still observes a transient status and `StopSearchJob`
/// can still accept it. Nothing sleeps for it: the due instant is stored and compared
/// against the clock.
pub const TRANSITION_DELAY: Duration = Duration::from_millis(500);

/// How long after `StartSearchJob` the clock owes the job [`RUNNING`], i.e. how long it
/// sits in [`CREATING`] first.
///
/// Also CHOSEN rather than modelled — AWS documents no duration for the set-up phase
/// either — at 250 ms, inside the same 250-750 ms band, and deliberately SHORTER than
/// [`TRANSITION_DELAY`], which is when the scan itself is owed its terminal status: both
/// transient phases a scan passes through are therefore observable, the first one by a
/// caller acting straight after the start call.
pub const CREATING_DELAY: Duration = Duration::from_millis(250);

/// How long a `ClientToken` of this service takes part in idempotency, measured from
/// the moment the request that first used it completed.
///
/// MODELLED rather than chosen: the member's own documentation on BOTH operations that
/// take one states the window and what happens after it — a token is valid for eight
/// hours after the first request using it completed, and any later request carrying it
/// is treated as a new request
/// (`smithy_query(query="doc", shape="StartSearchJobInput", member="ClientToken")`,
/// same text on `StartSearchResultExportJobInput`). So this is not one of the emulator's
/// own spans: it is the duration the service publishes.
pub const CLIENT_TOKEN_VALIDITY: Duration = Duration::from_secs(8 * 60 * 60);

/// A `ClientToken` a request carried, together with the instant its validity window
/// closes ([`CLIENT_TOKEN_VALIDITY`] after the request that used it completed).
///
/// The token and its window are ONE value, so no record can hold a token nothing
/// bounds: a stored token answers a repeat only while its window is open, and once the
/// clock has passed it the token names no earlier request at all — a call carrying it is
/// a new request, neither a repeat nor a conflict.
#[derive(Debug, Clone)]
pub struct ClientToken {
    value: String,
    expires_at: SystemTime,
}

impl ClientToken {
    /// The token `value` carried by a request that completed at `completed`, valid until
    /// [`CLIENT_TOKEN_VALIDITY`] after that instant.
    ///
    /// Adding a duration to a `SystemTime` panics when the sum is not representable, so
    /// the window's end is computed fallibly and an unrepresentable one is taken as a
    /// window that has ALREADY closed: a clock far enough ahead makes the next call under
    /// the token a new request rather than aborting the handler.
    pub fn minted(value: String, completed: SystemTime) -> Self {
        Self {
            value,
            expires_at: completed
                .checked_add(CLIENT_TOKEN_VALIDITY)
                .unwrap_or(completed),
        }
    }

    /// The token as the request spelled it.
    pub fn value(&self) -> &str {
        &self.value
    }

    /// Whether this token still takes part in idempotency at `now`. The window is open
    /// up to but NOT including the instant it closes, so a token exactly its full
    /// validity old is already past it.
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        now < self.expires_at
    }

    /// Whether this token is `value` AND still inside its window — which is what makes
    /// the record carrying it the answer to a repeat rather than an unrelated record.
    pub fn is_live(&self, value: &str, now: SystemTime) -> bool {
        self.value == value && self.is_valid_at(now)
    }
}

/// Drop a record's `ClientToken` once the clock has carried it past its validity window.
///
/// Materialising a change the CLOCK already made due, like every other transition this
/// store settles: the window closed at an instant that was reached whether or not
/// anybody called, and from then on the record no longer holds a token for anything to
/// match. Clearing it is what keeps a token from naming one job for the life of the
/// process.
fn expire_client_token(token: &mut Option<ClientToken>, now: SystemTime) {
    if token.as_ref().is_some_and(|held| !held.is_valid_at(now)) {
        *token = None;
    }
}

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

/// How many times an identifier mint draws a UUID while the one it drew is already held
/// (see [`ComAmazonawsBackupsearchCryobackupsearchserviceState::mint_identifier`]). A
/// UUIDv4 has 122 bits of entropy, so one re-draw is already past what this store can
/// expect to need; the bound is what makes the mint provably terminating.
const UUID_MINT_ATTEMPTS: usize = 8;

/// The server fault's text when a mint could not draw an identifier this store does not
/// already hold — a broken random source, which is a server problem and never a reason
/// to hand the caller a resource id another record already uses.
pub const MINT_EXHAUSTED: &str = "a unique job identifier could not be minted";

/// The ARN resource segment of an export job: `export-job/<ExportJobIdentifier>`.
pub const EXPORT_JOB_RESOURCE: &str = "export-job";

/// The ceiling on how many export jobs this store holds, for the same reason as
/// [`MAX_SEARCH_JOBS`]: `StartSearchResultExportJob` is the only thing that adds one and
/// nothing evicts them, so the operation that grows the store refuses past this cap with
/// the modelled `ServiceQuotaExceededException` (HTTP 402) the model declares on it. The
/// number is the emulator's own bound, not a published AWS default.
pub const MAX_EXPORT_JOBS: usize = 5_000;

/// The one wording of the server fault every handler answers with when this emulator's
/// search-job store cannot be reached, so the text cannot drift between operations.
pub const STORE_UNAVAILABLE: &str = "the search job store is unavailable";

/// The `StatusMessage` a search job whose scope resolved to no recovery point carries.
///
/// The model documents `StatusMessage` only as the text returned "for ... jobs with
/// issues", so it states no wording — but the service's recordings do: a `StartSearchJob`
/// whose `SearchScope` matches nothing is accepted and then settles to [`FAILED`] with
/// THIS sentence, which carries none of the request's data and is the same for every such
/// job. Being request-independent is what makes it the service's fixed template for this
/// outcome rather than one call's example text, so the settle path takes it from here.
pub const SCOPE_EMPTY: &str =
    "Search scope is empty. Revise search scope input and retry search job";

/// The text this service's `ResourceNotFoundException` carries when a job identifier
/// names nothing.
///
/// The model documents the member only as "Request references a resource which does not
/// exist", so it states no text — but the service's own recordings do: the SAME
/// request-independent sentence comes back from `GetSearchJob`, `ListSearchJobResults`,
/// `StopSearchJob` and `GetSearchResultExportJob` for a well-formed identifier naming
/// nothing. Being byte-identical across operations and carrying none of the request's
/// data is what makes it the service's fixed not-found template rather than one call's
/// example text, so every job-identifier lookup here refuses with it and none echoes the
/// identifier back. (The tag APIs, which resolve a resource by ARN, are recorded with a
/// different refusal and keep their own.)
pub const RESOURCE_NOT_FOUND: &str = "Resource is not found";

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

/// An `ExportJobArn`: `arn:aws:backup-search:<region>:<account>:export-job/<id>`.
pub fn export_job_arn(signing_name: &str, region: &str, identifier: &str) -> String {
    ::engine::emulator_arn(
        signing_name,
        region,
        &format!("{EXPORT_JOB_RESOURCE}/{identifier}"),
    )
}

/// The account segment of an ARN, which is the fifth colon-separated field
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`). `None` when the string
/// carries no such field.
pub fn arn_account(arn: &str) -> Option<&str> {
    arn.split(':').nth(4)
}

/// The resource TYPE an ARN names: the part of its resource field before the first `/`
/// or `:` separator (`search-job`, `export-job`), and the empty string when the resource
/// field carries no separator at all — a field like an S3 bucket name is an id alone and
/// claims no type — or when the string has no resource field.
///
/// This is what the tag APIs report as `ResourceNotFoundException$resourceType`, which
/// the model documents as the "hypothetical type of the resource affected" — the type the
/// SUBMITTED ARN claims, since no resource of it was found. Reading it off the ARN is
/// what keeps the refusal from naming one resource type for every miss, now that the
/// lookup spans more than one.
pub fn arn_resource_type(arn: &str) -> String {
    // `splitn(6, ..)` so the resource field keeps its own colons: an ARN is
    // `arn:<partition>:<service>:<region>:<account>:<resource>` and a resource field may
    // itself be `<type>:<id>`.
    arn.splitn(6, ':')
        .nth(5)
        .and_then(|resource| resource.split_once(['/', ':']))
        .map(|(resource_type, _)| resource_type.to_string())
        .unwrap_or_default()
}

/// Why this service cannot pass `role_arn` to itself, or `None` when it can.
///
/// Starting an export makes the service assume the submitted role to write the results,
/// so the role has to be passable to it. A role can only be passed by a principal in the
/// role's OWN account — AWS refuses a cross-account pass-role — and every caller of this
/// emulator is attributed to [`::engine::emulator_account_id`], so a `RoleArn` naming any
/// other account cannot be passed and the modelled `AccessDeniedException` (HTTP 403) is
/// the answer. A well-formed ARN is never turned away by input validation for this: the
/// refusal is an authorization decision, which is the class a caller branches on.
///
/// A string with no account field at all cannot be judged here and is not denied; the
/// `IamRoleArn` member's own `@pattern` already refuses it before any handler runs.
pub fn pass_role_denial(role_arn: &str) -> Option<String> {
    let account = arn_account(role_arn)?;
    (account != ::engine::emulator_account_id())
        .then(|| CROSS_ACCOUNT_PASS_ROLE.to_string())
}

/// The `AccessDeniedException` message for a role in another account. AWS states this
/// refusal in one fixed sentence, because the rule behind it names nothing from the
/// request: passing a role across accounts is not allowed at all.
pub const CROSS_ACCOUNT_PASS_ROLE: &str = "Cross-account pass role is not allowed.";

/// One persisted search job. Every member a modelled response carries is read from
/// here; nothing in a response is recomputed from the request.
#[derive(Debug, Clone)]
pub struct SearchJob {
    pub identifier: String,
    pub arn: String,
    pub name: Option<String>,
    /// One of [`TERMINAL_STATES`], [`CREATING`], [`RUNNING`] or [`STOPPING`].
    pub status: String,
    pub creation_time: DateTime,
    pub completion_time: Option<DateTime>,
    pub status_message: Option<String>,
    pub encryption_key_arn: Option<String>,
    pub search_scope: SearchScope,
    pub item_filters: ItemFilters,
    pub search_scope_summary: SearchScopeSummary,
    pub current_search_progress: CurrentSearchProgress,
    /// The matches the scan RECORDED. `ListSearchJobResults` pages over exactly this, so
    /// it is empty for a scan that has not got through its scope yet — the matches it will
    /// record wait in [`SearchJob::scan_matches`] until the clock makes the scan's
    /// completion due.
    pub results: Vec<ResultItem>,
    /// The matches the scan FINDS: the items held by the backup indexes of the recovery
    /// points in scope that this job's `ItemFilters` admit, resolved from the store when
    /// the job was started (the same instant the scope itself was resolved, so a recovery
    /// point indexed later does not change a running scan's outcome).
    ///
    /// [`SearchJob::settle`] moves them into [`SearchJob::results`] when the scan reaches
    /// [`COMPLETED`], which is the instant the progress reports them matched: a reader can
    /// page exactly the matches the progress counts, never more.
    pub scan_matches: Vec<ResultItem>,
    /// The recovery points the scan covered, each with the status its backup index had.
    /// `ListSearchJobBackups` pages over exactly this: the recovery points of this account
    /// the job's scope resolved to ([`ComAmazonawsBackupsearchCryobackupsearchserviceState::recovery_points_in_scope`]).
    pub backups: Vec<SearchJobBackupsResult>,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the call that minted this job carried, if any, with the instant
    /// its validity window closes. Idempotency is keyed off the job records themselves,
    /// so a caller cannot grow a second index by varying only the token; the window is
    /// what keeps that key from outliving the token
    /// ([`CLIENT_TOKEN_VALIDITY`]).
    pub client_token: Option<ClientToken>,
    /// Canonical form of the `StartSearchJob` body this job was created from. A repeat
    /// of the same body under the same `ClientToken` is idempotent; a different body
    /// under that token is a conflict.
    pub request_fingerprint: String,
    /// The statuses the clock owes this job IN DUE ORDER: each entry is the instant the
    /// status is owed from, the status, and the `StatusMessage` that status comes with
    /// (`None` for one that needs no message). A scan owes two — [`RUNNING`] and then its
    /// terminal status — so this is a queue and not a single transition; it is EMPTY for a
    /// job in a terminal status, which owes nothing.
    pub owed_statuses: VecDeque<(SystemTime, &'static str, Option<&'static str>)>,
}

impl SearchJob {
    /// Whether this job has reached a status it never leaves.
    pub fn is_terminal(&self) -> bool {
        TERMINAL_STATES.contains(&self.status.as_str())
    }

    /// Whether this job is already in, or already on its way to, the end state
    /// `StopSearchJob` asks for (see [`STOP_SATISFIED_STATES`]).
    pub fn is_stop_satisfied(&self) -> bool {
        STOP_SATISFIED_STATES.contains(&self.status.as_str())
    }

    /// Make `status` the ONE status this job is still owed, due at `due`, dropping
    /// whatever it was owed before.
    ///
    /// What a stop does to a scan: the statuses the scan's own schedule still held are no
    /// longer owed, so a stopped job cannot later settle to the terminal status its scan
    /// would have reached.
    pub fn owe_only(
        &mut self,
        due: SystemTime,
        status: &'static str,
        message: Option<&'static str>,
    ) {
        self.owed_statuses.clear();
        self.owed_statuses.push_back((due, status, message));
    }

    /// Apply every owed status transition the clock has already made due, in order — so a
    /// job nobody looked at while two of its spans passed reports the later status rather
    /// than catching up one call at a time.
    ///
    /// The scan's progress is brought to the scope it was given — a scan that ran to
    /// `COMPLETED` has scanned everything in scope and matched exactly the results it
    /// recorded; one that was `STOPPED` keeps the progress it had reached, and one that
    /// `FAILED` never got through its scope, so it keeps the progress it had too.
    ///
    /// The matches the scan found are RECORDED by the same transition
    /// ([`SearchJob::scan_matches`] -> [`SearchJob::results`]), so what
    /// `ListSearchJobResults` pages and what the progress counts as matched are written at
    /// one instant and can never disagree.
    fn settle(&mut self, now: SystemTime) {
        expire_client_token(&mut self.client_token, now);
        while let Some(&(due, next, message)) = self.owed_statuses.front() {
            if now < due {
                return;
            }
            self.owed_statuses.pop_front();
            self.status = next.to_string();
            // The status and the message that status comes with are written together, so
            // a record can never explain a failure it has not reached, nor reach one
            // silently.
            self.status_message = message.map(ToString::to_string);
            // `CompletionTime` is when the job COMPLETED, so only a status the job never
            // leaves sets it: passing through a transient status completes nothing.
            if TERMINAL_STATES.contains(&next) {
                self.completion_time = Some(timestamp(due));
            }
            if next == COMPLETED {
                // A scan that got through its scope has RECORDED the matches it found, so
                // they become readable at this instant and not before — and the progress
                // is written from the same record, so the matched count and the results a
                // reader pages cannot disagree.
                self.results = std::mem::take(&mut self.scan_matches);
                self.current_search_progress = CurrentSearchProgress {
                    recovery_points_scanned_count: self
                        .search_scope_summary
                        .total_recovery_points_to_scan_count,
                    items_scanned_count: self.search_scope_summary.total_items_to_scan_count,
                    items_matched_count: Some(self.results.len() as i64),
                };
            } else if TERMINAL_STATES.contains(&next) {
                // The scan ended without getting through its scope, so the matches it
                // would have recorded are no longer owed to anybody: it keeps whatever it
                // had already recorded and finds nothing more.
                self.scan_matches = Vec::new();
            }
        }
    }
}

/// One recovery point of this account's backup index, WITH the items that index holds.
///
/// A scan has two layers and this record carries both. The outer one is the recovery
/// point itself, which a `SearchScope` selects and `ListSearchJobBackups` reports. The
/// inner one is the items inside that backup's index — the objects of a backed-up S3
/// bucket, the files of a backed-up EBS volume — which is the layer a job's `ItemFilters`
/// select from, `SearchScopeSummary.TotalItemsToScanCount` counts and
/// `ListSearchJobResults` pages. Holding them on ONE record is what keeps a recovery
/// point and the items it holds from drifting apart.
#[derive(Debug, Clone, Default)]
pub struct IndexedRecoveryPoint {
    /// The recovery point itself, exactly as `ListSearchJobBackups` reports it.
    pub backup: SearchJobBackupsResult,
    /// The backup vault holding it. A result item reports the vault its backup lives in
    /// while `SearchJobBackupsResult` carries no vault member, so the name is held here:
    /// it is a property of the backup, not of one item inside it.
    pub backup_vault_name: Option<String>,
    /// The items this recovery point's backup index holds, in index order: an
    /// `S3ResultItem` per backed-up object (its key, size, creation time, ETag and
    /// version id) or an `EBSResultItem` per backed-up file (its path, size and times).
    ///
    /// The three members naming the BACKUP an item came from
    /// (`BackupResourceArn`/`SourceResourceArn`/`BackupVaultName`) belong to this record
    /// rather than to one item, so a scan stamps them from here as it reads the index
    /// ([`scanned_items`]) and a result cannot name a backup it was not found in.
    pub items: Vec<ResultItem>,
}

impl From<SearchJobBackupsResult> for IndexedRecoveryPoint {
    /// A recovery point whose backup index this store holds no items for, which is every
    /// recovery point until something writes its index: a scope resolving to it has a
    /// backup to scan and no item inside it.
    fn from(backup: SearchJobBackupsResult) -> Self {
        Self {
            backup,
            backup_vault_name: None,
            items: Vec::new(),
        }
    }
}

/// One persisted export job: the record `StartSearchResultExportJob` writes and
/// `GetSearchResultExportJob`/`ListSearchResultExportJobs` report. Every member of a
/// modelled response is read from here; nothing is recomputed from the request.
#[derive(Debug, Clone)]
pub struct ExportJob {
    pub identifier: String,
    pub arn: String,
    /// The search job whose results this export transmits.
    pub search_job_identifier: String,
    pub search_job_arn: String,
    /// One of the modelled `ExportJobStatus` values ([`RUNNING`], [`COMPLETED`],
    /// [`FAILED`]).
    pub status: String,
    pub creation_time: DateTime,
    pub completion_time: Option<DateTime>,
    /// The text a `FAILED` export carries, as the model documents the member. Written
    /// only once the export has actually reached that status — the text the clock owes
    /// lives in [`ExportJob::next_status_due`] until then.
    pub status_message: Option<String>,
    pub export_specification: ExportSpecification,
    /// The `RoleArn` the start call carried, if any — stored so the record says which
    /// role the export was started under.
    pub role_arn: Option<String>,
    pub tags: HashMap<String, String>,
    /// The `ClientToken` the call that minted this export job carried, if any, with the
    /// instant its validity window closes ([`CLIENT_TOKEN_VALIDITY`]).
    pub client_token: Option<ClientToken>,
    /// Canonical form of the `StartSearchResultExportJob` body this job was created
    /// from: a repeat under the same `ClientToken` is idempotent, a different body under
    /// that token is a conflict.
    pub request_fingerprint: String,
    /// The status the clock owes this export job, the instant it is owed from, and the
    /// `StatusMessage` that status comes with (`None` for one that needs no message).
    /// `None` once it has reached a status it never leaves.
    pub next_status_due: Option<(SystemTime, &'static str, Option<String>)>,
}

impl ExportJob {
    /// Apply the owed status transition if the clock has already made it due.
    fn settle(&mut self, now: SystemTime) {
        expire_client_token(&mut self.client_token, now);
        let Some((due, next, message)) = self.next_status_due.clone() else {
            return;
        };
        if now < due {
            return;
        }
        self.status = next.to_string();
        self.next_status_due = None;
        self.completion_time = Some(timestamp(due));
        // The status and the message it comes with are written together, so a record can
        // never report a failure's text while it is still RUNNING.
        self.status_message = message;
    }
}

/// Per-service state for com-amazonaws-backupsearch-cryobackupsearchservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBackupsearchCryobackupsearchserviceState {
    /// Every search job, keyed by its `SearchJobIdentifier`.
    pub search_jobs: HashMap<String, SearchJob>,
    /// Identifiers in creation order, so `ListSearchJobs` pages deterministically.
    /// `ListSearchJobs` walks this backwards (see
    /// [`jobs_newest_first`](Self::jobs_newest_first)).
    pub job_order: Vec<String>,
    /// The recovery points of this account a search job's `SearchScope` is resolved
    /// against, each with the items its backup index holds — the backup index the Backup
    /// data plane builds, which is what a scan actually scans, down to the item layer
    /// ([`IndexedRecoveryPoint`]).
    ///
    /// A recovery point exists for this account only once that data plane holds one, and
    /// nothing in THIS service creates one — no operation it models takes a backup or
    /// indexes one — so in a running emulator this stays EMPTY: every scope resolves to
    /// nothing and every scan therefore settles to the `FAILED`/[`SCOPE_EMPTY`] outcome
    /// the service answers for an empty scope, which is exactly what the recordings show.
    /// The `COMPLETED` branch of [`scan_outcome`] is reached as soon as this holds a
    /// recovery point the scope selects: a test seeds them, and a future Backup data
    /// plane in this emulator would write them here rather than teach the scan a second
    /// source. BOTH layers a job reports are COUNTED from here — the scope from
    /// [`recovery_points_in_scope`](Self::recovery_points_in_scope) and the items from
    /// [`scanned_items`] — rather than assumed by a handler, so every member a job reports
    /// about its scope and its progress, and the terminal status it settles to, follows the
    /// records this store holds.
    pub recovery_points: Vec<IndexedRecoveryPoint>,
    /// Every export job, keyed by its `ExportJobIdentifier`.
    pub export_jobs: HashMap<String, ExportJob>,
    /// Export-job identifiers in creation order, so `ListSearchResultExportJobs` pages
    /// deterministically. `ListSearchResultExportJobs` walks this backwards (see
    /// [`export_jobs_newest_first`](Self::export_jobs_newest_first)), exactly as
    /// `ListSearchJobs` walks `job_order`.
    pub export_job_order: Vec<String>,
}

/// The records `order` names, NEWEST-CREATED FIRST.
///
/// `order` holds identifiers in creation order and is only ever appended to, so walking
/// it backwards is exactly newest-first and is stable across calls; an identifier the
/// map no longer holds is skipped rather than reported as a gap.
///
/// ONE helper behind BOTH job listings of this service
/// ([`ComAmazonawsBackupsearchCryobackupsearchserviceState::jobs_newest_first`] and
/// [`ComAmazonawsBackupsearchCryobackupsearchserviceState::export_jobs_newest_first`]),
/// so the two cannot drift apart on the order the service reports its jobs in.
fn newest_first<'a, T>(
    order: &'a [String],
    records: &'a HashMap<String, T>,
) -> impl Iterator<Item = &'a T> {
    order
        .iter()
        .rev()
        .filter_map(|identifier| records.get(identifier))
}

impl ComAmazonawsBackupsearchCryobackupsearchserviceState {
    /// Materialise every status transition the clock has already made due. Call this
    /// FIRST in every handler that reports or moves a search job's STATUS, including a
    /// read: it only applies a change that was owed at `now` whether or not anyone
    /// called. The tagging operations (`TagResource`, `UntagResource`,
    /// `ListTagsForResource`) are exempt — they reach a job's tag set by ARN, which no
    /// owed status transition touches, so settling could not change what they read or
    /// write.
    pub fn settle(&mut self, now: SystemTime) {
        for job in self.search_jobs.values_mut() {
            job.settle(now);
        }
        for job in self.export_jobs.values_mut() {
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

    /// The tag set stored on the resource whose minted ARN is `arn` — how the tagging
    /// operations, which take an unconstrained `ResourceArn`, resolve the resource they
    /// name.
    ///
    /// The lookup spans EVERY resource this service mints and holds, not one type of
    /// them: a `ResourceArn` is documented only as "the resource that will have the
    /// indicated tags", and both of this service's resources are its own
    /// (`SearchJobArn` and `ExportJobArn` are each `@arnReference` to this service) and
    /// both carry the same `TagMap` on the operation that mints them. So an ARN matching
    /// a held search job or a held export job resolves, and `None` — the
    /// `ResourceNotFoundException` the tag APIs answer — is reserved for an ARN that
    /// matches no record this store holds.
    pub fn tags_by_arn(&self, arn: &str) -> Option<&HashMap<String, String>> {
        self.search_jobs
            .values()
            .find(|job| job.arn == arn)
            .map(|job| &job.tags)
            .or_else(|| {
                self.export_jobs
                    .values()
                    .find(|job| job.arn == arn)
                    .map(|job| &job.tags)
            })
    }

    /// The tag set stored on the resource whose minted ARN is `arn`, mutably — the same
    /// resolution [`tags_by_arn`](Self::tags_by_arn) performs, for `TagResource` and
    /// `UntagResource`.
    pub fn tags_by_arn_mut(&mut self, arn: &str) -> Option<&mut HashMap<String, String>> {
        // Resolved to an identifier first, then borrowed mutably: the map a record lives
        // in cannot be borrowed for the search and the write at once.
        if let Some(identifier) = self
            .search_jobs
            .values()
            .find(|job| job.arn == arn)
            .map(|job| job.identifier.clone())
        {
            return self
                .search_jobs
                .get_mut(&identifier)
                .map(|job| &mut job.tags);
        }
        let identifier = self
            .export_jobs
            .values()
            .find(|job| job.arn == arn)
            .map(|job| job.identifier.clone())?;
        self.export_jobs
            .get_mut(&identifier)
            .map(|job| &mut job.tags)
    }

    /// Every job NEWEST-CREATED FIRST, which is the sequence `ListSearchJobs` filters
    /// and pages: the service reports the most recently created job first, so a
    /// single-page read is "the latest search job".
    ///
    /// `job_order` holds the identifiers in creation order and is only ever appended to,
    /// so walking it backwards is exactly newest-first and is stable across calls.
    /// `job_order` and `search_jobs` are written together, so an identifier in the order
    /// always resolves.
    pub fn jobs_newest_first(&self) -> impl Iterator<Item = &SearchJob> {
        newest_first(&self.job_order, &self.search_jobs)
    }

    /// The recovery points this store holds that `scope` selects — the scan's resolved
    /// scope, and so what a search job over that scope has to get through. Each one
    /// carries the items its backup index holds, which is the layer
    /// [`scanned_items`] reads.
    ///
    /// A recovery point is in scope when its resource type is one the scope names and
    /// every filter the scope carries admits it; a filter the records cannot be judged
    /// against (a tag filter: a recovery point record here carries no tags, nor does the
    /// model give it any) admits none, because this store holds nothing that can be
    /// shown to match it.
    pub fn recovery_points_in_scope(&self, scope: &SearchScope) -> Vec<IndexedRecoveryPoint> {
        self.recovery_points
            .iter()
            .filter(|point| in_scope(&point.backup, scope))
            .cloned()
            .collect()
    }

    /// The job the `StartSearchJob` call carrying `token` minted, if that call was made
    /// and its token is still inside its validity window at `now`. Read off the job
    /// records, so the idempotency index cannot outgrow [`MAX_SEARCH_JOBS`].
    ///
    /// A token whose window has closed matches NOTHING here, so the call carrying it is
    /// a new request: it mints a job, and a body differing from the expired call's is no
    /// longer a conflict. At most one held record can carry a live token of a given
    /// value, because minting another under it requires that no live one exists.
    pub fn job_by_client_token(&self, token: &str, now: SystemTime) -> Option<&SearchJob> {
        self.search_jobs
            .values()
            .find(|job| job.client_token.as_ref().is_some_and(|held| held.is_live(token, now)))
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

    /// A fresh `SearchJobIdentifier` no job in this store holds, or `None` when no free
    /// one could be drawn (see [`fresh_identifier`](Self::fresh_identifier)), which the
    /// caller answers as a server fault rather than by handing out a held identifier.
    ///
    /// `GenericId` carries no pattern or length in the model, but the service's
    /// recordings show what it mints: a UUID, which is also the only shape the service's
    /// own job-identifier guard accepts. So this draws a version-4 UUID — unique, opaque,
    /// not derived from the request and not reproducible across runs, which is what keeps
    /// two independently started jobs from colliding on id. The caller STORES what this
    /// returns; nothing recomputes it.
    pub fn mint_identifier(&self) -> Option<String> {
        let sequence = self.search_jobs.len() as u64;
        self.fresh_identifier(sequence, |store, candidate| {
            store.search_jobs.contains_key(candidate)
        })
    }

    /// The export job `identifier` names, or `None` when this account holds no such
    /// export job.
    pub fn export_job(&self, identifier: &str) -> Option<&ExportJob> {
        self.export_jobs.get(identifier)
    }

    /// Every export job NEWEST-CREATED FIRST, which is the sequence
    /// `ListSearchResultExportJobs` filters and pages — the SAME order
    /// [`jobs_newest_first`](Self::jobs_newest_first) reports a search job listing in,
    /// because the rule is the service's and not one listing's: a job listing of this
    /// service reports the most recently created job first, so a single-page read is
    /// "the latest export job".
    ///
    /// `export_job_order` and `export_jobs` are written together, so an identifier in
    /// the order always resolves.
    pub fn export_jobs_newest_first(&self) -> impl Iterator<Item = &ExportJob> {
        newest_first(&self.export_job_order, &self.export_jobs)
    }

    /// The export job the `StartSearchResultExportJob` call carrying `token` minted, if
    /// that call was made and its token is still inside its validity window at `now` —
    /// the same rule [`job_by_client_token`](Self::job_by_client_token) applies, because
    /// the model states it on both of this service's `ClientToken` members. Read off the
    /// records themselves, so the idempotency index cannot outgrow [`MAX_EXPORT_JOBS`].
    pub fn export_job_by_client_token(&self, token: &str, now: SystemTime) -> Option<&ExportJob> {
        self.export_jobs
            .values()
            .find(|job| job.client_token.as_ref().is_some_and(|held| held.is_live(token, now)))
    }

    /// Whether this store is already holding [`MAX_EXPORT_JOBS`] export jobs, which is
    /// when `StartSearchResultExportJob` must refuse rather than mint another.
    pub fn export_jobs_full(&self) -> bool {
        self.export_jobs.len() >= MAX_EXPORT_JOBS
    }

    /// Store a newly minted export job, keeping the creation order.
    pub fn insert_export_job(&mut self, job: ExportJob) {
        self.export_job_order.push(job.identifier.clone());
        self.export_jobs.insert(job.identifier.clone(), job);
    }

    /// A fresh `ExportJobIdentifier` no export job in this store holds, or `None` when
    /// no free one could be drawn.
    ///
    /// Minted exactly like [`mint_identifier`](Self::mint_identifier), and for the same
    /// reason: both members are the model's `GenericId`, both are minted by this one
    /// service, and what it mints for a job is a UUID.
    pub fn mint_export_identifier(&self) -> Option<String> {
        let sequence = self.export_jobs.len() as u64;
        self.fresh_identifier(sequence, |store, candidate| {
            store.export_jobs.contains_key(candidate)
        })
    }

    /// A version-4 UUID this store does not already hold, as `taken` decides what
    /// "hold" means. One mint for every identifier this service hands out, so two of
    /// them cannot disagree about the shape of an id.
    ///
    /// A UUIDv4 collides with 122 bits of entropy behind it, so the re-draw is BOUNDED
    /// rather than a `loop`: the function terminates on its own. Exhausting the bound
    /// would mean a broken random source, and the answer is then `None` — never an
    /// identifier the store already holds, which a caller would insert over an existing
    /// record.
    fn fresh_identifier(
        &self,
        sequence: u64,
        taken: impl Fn(&Self, &str) -> bool,
    ) -> Option<String> {
        (0..UUID_MINT_ATTEMPTS)
            .map(|attempt| new_uuid(sequence.wrapping_add(attempt as u64)))
            .find(|candidate| !taken(self, candidate))
    }
}

/// A fresh identifier in the shape this service's recordings show it mints: a version-4
/// UUID, `8-4-4-4-12` lowercase hex with the version and variant bits RFC 4122 §4.4
/// fixes.
///
/// The 128 bits come from three sources hashed together with SHA-256 (the `sha2` crate)
/// and hex-encoded (the `hex` crate): the OS-seeded randomness a `RandomState` draws, the
/// system clock in nanoseconds, and `sequence` — a number the store varies per draw, so
/// two draws in the same nanosecond cannot collide. Nothing here comes from the request,
/// and nothing is reproducible across runs. An id names a resource for correlation; a
/// credential would come from `ctx.credential_store()` instead, never from here.
fn new_uuid(sequence: u64) -> String {
    use ::sha2::{Digest, Sha256};
    use std::collections::hash_map::RandomState;
    use std::hash::BuildHasher;

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(RandomState::new().hash_one(sequence).to_le_bytes());
    hasher.update(nanos.to_le_bytes());
    hasher.update(sequence.to_le_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version nibble is 4, and the variant's two top bits are `10`.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Whether `point` is one of the recovery points `scope` selects.
///
/// Every member of `SearchScope` is a filter over the backup index: the resource type
/// (the model allows exactly one), the recovery-point and source-resource ARNs, the
/// window the backup was created in, and the backup's tags. A filter the request leaves
/// out admits everything; a filter it carries admits only what matches it, and a record
/// that cannot be judged against a filter it carries (no creation time against a creation
/// window, no tags at all against a tag filter) is not selected, because nothing shows it
/// to be in scope.
fn in_scope(point: &SearchJobBackupsResult, scope: &SearchScope) -> bool {
    // `BackupResourceTypes` is @required and @length(min: 1, max: 1), so it always names
    // a type and is never "no filter": a record whose type it does not name is out of
    // scope. (The ARN lists below read an EMPTY list the other way round, as no filter at
    // all, because the model makes them optional with @length(min: 0).)
    if !scope
        .backup_resource_types
        .iter()
        .any(|wanted| point.resource_type.as_deref() == Some(wanted.as_str()))
    {
        return false;
    }
    if !arn_admits(scope.backup_resource_arns.as_deref(), point.backup_resource_arn.as_deref()) {
        return false;
    }
    if !arn_admits(scope.source_resource_arns.as_deref(), point.source_resource_arn.as_deref()) {
        return false;
    }
    if let Some(window) = scope.backup_resource_creation_time.as_ref() {
        let Some(created) = point.backup_creation_time.map(instant_key) else {
            return false;
        };
        // "only created after"/"only created before" the given instant, with the bound
        // itself in scope: the filter names the edge of the window, not a point outside it.
        if window
            .created_after
            .is_some_and(|after| created < instant_key(after))
            || window
                .created_before
                .is_some_and(|before| created > instant_key(before))
        {
            return false;
        }
    }
    // A tag filter: a recovery point carries no tags in this model, so no record this
    // store holds can be shown to carry the ones the scope asks for. Revisit this branch
    // if `SearchJobBackupsResult` ever gains a tags member (or the records written into
    // `recovery_points` carry tags some other way): it would then have to MATCH the
    // filter rather than exclude everything.
    scope
        .backup_resource_tags
        .as_ref()
        .is_none_or(|tags| tags.is_empty())
}

/// Whether an ARN filter admits `candidate`: an absent or empty list is no filter at all,
/// and a list admits exactly the ARNs it names.
fn arn_admits(filter: Option<&[String]>, candidate: Option<&str>) -> bool {
    match filter {
        None => true,
        Some([]) => true,
        Some(arns) => candidate.is_some_and(|arn| arns.iter().any(|wanted| wanted == arn)),
    }
}

/// A `DateTime` as a totally ordered key, so two instants can be compared: the model's
/// timestamps carry seconds and sub-second nanos, and `DateTime` itself is not `Ord`.
fn instant_key(instant: DateTime) -> (i64, u32) {
    (instant.secs(), instant.subsec_nanos())
}

/// The terminal status the clock owes a scan over `recovery_points_in_scope` recovery
/// points, with the `StatusMessage` that status comes with.
///
/// A scan that had something to get through ends `COMPLETED` and explains nothing. A
/// scope that resolved to NO recovery point is a terminal failure — there was nothing to
/// search — and the service says so in the message, so a caller branching on a terminal
/// status takes its failure path instead of reading results that were never produced.
pub fn scan_outcome(recovery_points_in_scope: usize) -> (&'static str, Option<&'static str>) {
    if recovery_points_in_scope == 0 {
        (FAILED, Some(SCOPE_EMPTY))
    } else {
        (COMPLETED, None)
    }
}

/// The items the backup indexes of `points` hold, in recovery-point order and then index
/// order, each naming the backup it was found in.
///
/// This is the ITEM layer of a scan: what a job over that resolved scope has to get
/// through (`SearchScopeSummary.TotalItemsToScanCount`,
/// `CurrentSearchProgress.ItemsScannedCount`) and the sequence its `ItemFilters` select
/// the matches from. The three members naming the backup are stamped from the recovery
/// point record the item was read out of, so a result never names a backup it was not
/// found in and never reports a vault the record does not hold.
pub fn scanned_items(points: &[IndexedRecoveryPoint]) -> Vec<ResultItem> {
    points
        .iter()
        .flat_map(|point| point.items.iter().map(|item| item_of_backup(item, point)))
        .collect()
}

/// `item` as the scan read it out of `point`'s backup index: its own properties, with the
/// backup it was found in named from that record.
fn item_of_backup(item: &ResultItem, point: &IndexedRecoveryPoint) -> ResultItem {
    let backup_resource_arn = point.backup.backup_resource_arn.clone();
    let source_resource_arn = point.backup.source_resource_arn.clone();
    let backup_vault_name = point.backup_vault_name.clone();
    match item.clone() {
        ResultItem::S3ResultItem(object) => ResultItem::S3ResultItem(S3ResultItem {
            backup_resource_arn,
            source_resource_arn,
            backup_vault_name,
            ..object
        }),
        ResultItem::EBSResultItem(file) => ResultItem::EBSResultItem(EBSResultItem {
            backup_resource_arn,
            source_resource_arn,
            backup_vault_name,
            ..file
        }),
    }
}

/// Whether `filters` admit `item`, i.e. whether a scan over the item layer records it as
/// a match.
///
/// `ItemFilters` carries one list per item type and each list holds up to ten filter
/// objects, so the rule has three levels: an item is admitted when ANY filter object of
/// the list for ITS OWN type matches it, a filter object matches when EVERY item property
/// it constrains is satisfied, and a property is satisfied by ANY ONE of the conditions
/// listed for it — which is what the model documents for every one of those condition
/// lists ("if one value is included, the results will return only items that match the
/// value. If more than one value is included, the results will return all items that
/// match any of the values").
///
/// A list that is absent or empty constrains nothing and admits every item, exactly as an
/// absent scope filter admits every recovery point — and so does the list for the OTHER
/// item type, which says nothing about items of this one.
pub fn item_matches_filters(item: &ResultItem, filters: &ItemFilters) -> bool {
    match item {
        ResultItem::S3ResultItem(object) => {
            satisfied_by_any(filters.s3_item_filters.as_deref(), |filter| {
                s3_filter_matches(filter, object)
            })
        }
        ResultItem::EBSResultItem(file) => {
            satisfied_by_any(filters.ebs_item_filters.as_deref(), |filter| {
                ebs_filter_matches(filter, file)
            })
        }
    }
}

/// Whether one `S3ItemFilter` matches a backed-up object: every property the filter
/// constrains is satisfied, and a property it leaves out constrains nothing.
fn s3_filter_matches(filter: &S3ItemFilter, object: &S3ResultItem) -> bool {
    satisfied_by_any(filter.object_keys.as_deref(), |condition| {
        string_condition_met(condition, object.object_key.as_deref())
    }) && satisfied_by_any(filter.sizes.as_deref(), |condition| {
        long_condition_met(condition, object.object_size)
    }) && satisfied_by_any(filter.creation_times.as_deref(), |condition| {
        time_condition_met(condition, object.creation_time)
    }) && satisfied_by_any(filter.version_ids.as_deref(), |condition| {
        string_condition_met(condition, object.version_id.as_deref())
    }) && satisfied_by_any(filter.e_tags.as_deref(), |condition| {
        string_condition_met(condition, object.e_tag.as_deref())
    })
}

/// Whether one `EBSItemFilter` matches a backed-up file, by the same rule
/// [`s3_filter_matches`] applies to an object.
fn ebs_filter_matches(filter: &EBSItemFilter, file: &EBSResultItem) -> bool {
    satisfied_by_any(filter.file_paths.as_deref(), |condition| {
        string_condition_met(condition, file.file_path.as_deref())
    }) && satisfied_by_any(filter.sizes.as_deref(), |condition| {
        long_condition_met(condition, file.file_size)
    }) && satisfied_by_any(filter.creation_times.as_deref(), |condition| {
        time_condition_met(condition, file.creation_time)
    }) && satisfied_by_any(filter.last_modification_times.as_deref(), |condition| {
        time_condition_met(condition, file.last_modified_time)
    })
}

/// Whether an optional list of ALTERNATIVES is satisfied: an absent or empty list states
/// no requirement at all, and a list that states one is satisfied by any ONE of its
/// entries. One helper behind both levels of the item filters that read this way — the
/// filter objects of a list, and the conditions of one property.
fn satisfied_by_any<T>(alternatives: Option<&[T]>, met: impl Fn(&T) -> bool) -> bool {
    alternatives.is_none_or(|list| list.is_empty() || list.iter().any(met))
}

/// Whether `candidate` satisfies one `StringCondition`.
///
/// A property the item does not carry cannot be SHOWN to satisfy a condition on it, so it
/// satisfies none — the same rule the scope resolution applies to a record it cannot
/// judge. An operator outside the model's enum is likewise not satisfied (restJson1 enums
/// are open on the wire, so an unknown value reaches here) rather than silently admitting
/// every item.
fn string_condition_met(condition: &StringCondition, candidate: Option<&str>) -> bool {
    let Some(candidate) = candidate else {
        return false;
    };
    let value = condition.value.as_str();
    match condition_operator(&condition.operator) {
        EQUALS_TO => candidate == value,
        NOT_EQUALS_TO => candidate != value,
        CONTAINS => candidate.contains(value),
        DOES_NOT_CONTAIN => !candidate.contains(value),
        BEGINS_WITH => candidate.starts_with(value),
        DOES_NOT_BEGIN_WITH => !candidate.starts_with(value),
        ENDS_WITH => candidate.ends_with(value),
        DOES_NOT_END_WITH => !candidate.ends_with(value),
        _ => false,
    }
}

/// Whether `candidate` satisfies one `LongCondition` — an item's size, by the operators
/// `LongConditionOperator` declares. Judged like [`string_condition_met`]: a size the item
/// does not carry satisfies nothing.
fn long_condition_met(condition: &LongCondition, candidate: Option<i64>) -> bool {
    let Some(candidate) = candidate else {
        return false;
    };
    match condition_operator(&condition.operator) {
        EQUALS_TO => candidate == condition.value,
        NOT_EQUALS_TO => candidate != condition.value,
        LESS_THAN_EQUAL_TO => candidate <= condition.value,
        GREATER_THAN_EQUAL_TO => candidate >= condition.value,
        _ => false,
    }
}

/// Whether `candidate` satisfies one `TimeCondition` — an item's creation or last
/// modification instant, by the operators `TimeConditionOperator` declares. The two
/// instants are compared as ordered keys ([`instant_key`]), so sub-second precision
/// counts.
fn time_condition_met(condition: &TimeCondition, candidate: Option<DateTime>) -> bool {
    let Some(candidate) = candidate.map(instant_key) else {
        return false;
    };
    let value = instant_key(condition.value);
    match condition_operator(&condition.operator) {
        EQUALS_TO => candidate == value,
        NOT_EQUALS_TO => candidate != value,
        LESS_THAN_EQUAL_TO => candidate <= value,
        GREATER_THAN_EQUAL_TO => candidate >= value,
        _ => false,
    }
}

/// The operator a condition compares with: the one the request spelled, or the model's
/// `@default("EQUALS_TO")` when it left the member out — the framework leaves an absent
/// enum member as the empty string, which is not an operator.
fn condition_operator(operator: &str) -> &str {
    if operator.is_empty() {
        EQUALS_TO
    } else {
        operator
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

/// One page of `items` starting at `offset`, with the token that resumes after it. The
/// token carries `context`, the sequence it indexes.
///
/// A page carries a `NextToken` whenever it is FULL — it holds as many entries as the
/// requested page size — because a full page is all this call was asked for and says
/// nothing about what follows it. A short page is the end of the sequence and carries
/// none, which is how a paginating client knows to stop: it follows the token until a
/// page comes back short, and the page a token at the very end of the sequence resumes
/// is the empty one.
pub fn page<T: Clone>(
    items: &[T],
    offset: usize,
    max_results: usize,
    context: &str,
) -> (Vec<T>, Option<String>) {
    let end = offset.saturating_add(max_results).min(items.len());
    let page = items[offset.min(items.len())..end].to_vec();
    let next = (page.len() == max_results).then(|| encode_next_token(context, end));
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

/// The sequence a `ListSearchJobBackups` page indexes: one search job's backups.
pub fn backups_page_context(identifier: &str) -> String {
    format!("backups:{identifier}")
}

/// The sequence a `ListSearchResultExportJobs` page indexes: the export jobs left by
/// both of that request's filters. Part of the page's `NextToken` for the same reason
/// [`jobs_page_context`] is: an offset alone does not say which filtered sequence it
/// indexes.
pub fn export_jobs_page_context(by_status: Option<&str>, by_search_job: Option<&str>) -> String {
    format!(
        "export-jobs:{}:{}",
        by_status.unwrap_or("ALL"),
        by_search_job.unwrap_or("ALL")
    )
}

/// A canonical rendering of one request's body, stored with the resource it created so a
/// later call under the same `ClientToken` can be told apart: the same body is a repeat,
/// a different body is a conflict.
///
/// The body is re-serialized with the protocol's own JSON codec — so exactly the
/// modelled members are compared, however the client sent them — with every object's
/// members put in key order, because a JSON object's member order is not part of the
/// request while a list's order is. One helper for every `Start` operation, so two
/// idempotency checks cannot disagree about what "the same body" means.
pub fn request_fingerprint(
    schema: &'static ::aws_smithy_schema::Schema,
    input: &dyn ::aws_smithy_schema::serde::SerializableStruct,
) -> Result<String, String> {
    use ::aws_smithy_schema::codec::{Codec, FinishSerializer};
    use ::aws_smithy_schema::serde::ShapeSerializer;

    let mut serializer =
        ::aws_smithy_schema_server::json::JsonCodec::rest_json().create_serializer();
    serializer
        .write_struct(schema, input)
        .map_err(|error| error.to_string())?;
    let json = serializer.finish();
    let value: ::serde_json::Value =
        ::serde_json::from_slice(&json).map_err(|error| error.to_string())?;
    ::serde_json::to_string(&key_ordered(&value)).map_err(|error| error.to_string())
}

/// `value` rebuilt with every object's members in key order, so two bodies differing
/// only in member order render identically.
///
/// The ordering is imposed here rather than left to `serde_json::Map`, whose iteration
/// order is sorted only while nothing in the build enables the crate's `preserve_order`
/// feature; inserting in sorted order is correct either way.
fn key_ordered(value: &::serde_json::Value) -> ::serde_json::Value {
    match value {
        ::serde_json::Value::Object(members) => {
            let ordered: ::std::collections::BTreeMap<String, ::serde_json::Value> = members
                .iter()
                .map(|(name, member)| (name.clone(), key_ordered(member)))
                .collect();
            ::serde_json::Value::Object(ordered.into_iter().collect())
        }
        ::serde_json::Value::Array(elements) => {
            ::serde_json::Value::Array(elements.iter().map(key_ordered).collect())
        }
        scalar => scalar.clone(),
    }
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
