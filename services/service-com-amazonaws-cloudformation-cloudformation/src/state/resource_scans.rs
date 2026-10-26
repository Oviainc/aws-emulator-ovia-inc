// SPDX-License-Identifier: CC0-1.0

//! RESOURCE SCANS: the scans of the resources in this account and Region, which
//! `ListResourceScans` reports summaries of.
//!
//! A resource scan is a record of its own, not a stack's: a scan "of the resources in this
//! account in this Region" is started for the account, and "you can [get] the status of a
//! scan using the ListResourceScans API action". Everything a summary reports therefore
//! lives on ONE [`StoredResourceScan`], so the read computes its answer from the record
//! rather than from the request: the scan's id, its status and status reason, when it
//! started and finished, how far it got, and which scan type it was.
//!
//! ## One namespace per Region
//!
//! A scan covers "the resources in this account in this Region", so it belongs to the
//! Region the call that started it was signed for ([`StoredResourceScan::region`]) and a
//! read signed for one Region reports nothing another Region holds — the same rule every
//! other read of this service follows ([`crate::state`]).
//!
//! ## The scan rides the clock
//!
//! A scan is ASYNCHRONOUS: the call that starts it is answered while the scan is still
//! running (the modelled `ResourceScanStatus` has an `IN_PROGRESS` value whose documentation
//! is "the resource scan is still in progress") and it reaches `COMPLETE` a moment later. A
//! record therefore carries the terminal status the clock owes in
//! [`StoredResourceScan::pending`], and
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::settle`] is what
//! materialises it: a caller listing straight after the call still sees the scan in flight,
//! one listing after the due instant sees it complete, and nothing sleeps or advances a scan
//! because it was read.
//!
//! ## What writes it
//!
//! `StartResourceScan` is what starts a scan, and
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::record_resource_scan`] is
//! the ONE way a record enters the store — so that operation, and the suites that cover the
//! reads over records a bound's worth of wire calls cannot create, both write through it
//! rather than reaching into the vector.
//!
//! ## What a scan READS
//!
//! The resources of this account and Region that this service holds a record of: every
//! resource of every live stack of that Region
//! ([`crate::state::ComAmazonawsCloudformationCloudformationState::scannable_resources`]).
//! Each one is therefore managed by a CloudFormation stack, which is what `ScannedResource`'s
//! `ManagedByStack` reports of it, and the stack that manages it is recorded on the row.
//!
//! A scan is a POINT-IN-TIME read: the rows are taken when the scan starts and nothing
//! recomputes them afterwards, so a resource created after that instant belongs to the next
//! scan rather than to this one — and a read of the record cannot change what it reports.
//!
//! ## One scan at a time
//!
//! "Only one can be run at a time for an account in a Region" — the modelled
//! `ResourceScanInProgressException`. The set that refusal is metered over is the records
//! [`StoredResourceScan::is_in_flight`] answers true for, in the Region the request was
//! signed for, so the refusal LIFTS on its own as the scan it counted completes.

use std::time::{Duration, SystemTime};

/// How many scans this store keeps, oldest reclaimed first.
///
/// Every growth site in this service's store is bounded, and a caller chooses how many
/// scans it starts. The modelled `ResourceScanLimitExceededException` documents a daily
/// quota without a number ("Exceeded the daily quota for resource scans"), so this is a
/// COUNT on the store rather than a claim about the real service's quota; it sits far above
/// any client workflow.
pub const MAX_RESOURCE_SCANS: usize = 200;

/// The model's `ResourceScanStatus` values, documented value by value on
/// `ResourceScanSummary$Status`: "IN_PROGRESS: The resource scan is still in progress.
/// COMPLETE: The resource scan is complete. EXPIRED: The resource scan has expired. FAILED:
/// The resource scan has failed."
pub mod status {
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    pub const COMPLETE: &str = "COMPLETE";
    pub const EXPIRED: &str = "EXPIRED";
    pub const FAILED: &str = "FAILED";
}

/// The model's `ScanType` enum (`smithy_query(query="shape", shape="ScanType")`): the scan
/// type a scan was run as, and the one a request filters on.
pub mod scan_type {
    pub const FULL: &str = "FULL";
    pub const PARTIAL: &str = "PARTIAL";
}

/// The scan type a request that names no `ScanTypeFilter` selects:
/// "The scan type that you want to get summary information about. The default is FULL."
pub const DEFAULT_SCAN_TYPE_FILTER: &str = scan_type::FULL;

/// How many summaries a response carries when the request names no `MaxResults`:
/// "The default value is 10."
pub const DEFAULT_MAX_RESULTS: usize = 10;

/// The most summaries one response carries, whatever `MaxResults` asks for:
/// "The maximum value is 100."
pub const MAX_RESULTS_CEILING: usize = 100;

/// How long after the call that starts it a scan's completion becomes due.
///
/// Chosen (nothing documents how long scanning an account's resources takes): non-zero, so
/// a caller listing straight after the call still sees the scan `IN_PROGRESS`, and short
/// enough that the list after it sees the scan complete.
pub const SCAN_DELAY: Duration = Duration::from_millis(500);

/// `PercentageCompleted` of a scan that has finished: "the percentage of the resource scan
/// that has been completed".
pub const PERCENTAGE_COMPLETE: f64 = 100.0;

/// `PercentageCompleted` of a scan that has only just started.
pub const PERCENTAGE_STARTED: f64 = 0.0;

/// The wildcard a resource-type filter may end with: "To scan all resource types within a
/// service, you can use a wildcard, represented by an asterisk (`*`). You can place an
/// asterisk at only the end of the string, for example, `AWS::S3::*`"
/// (`ScanFilter$Types`).
pub const TYPE_FILTER_WILDCARD: char = '*';

/// One resource a scan FOUND: what identifies it, and the stack that manages it.
///
/// `DescribeResourceScan` reports the COUNT of these rows and the distinct types among them;
/// the per-resource members are what a row IS, and `ListResourceScanResources` — still a
/// scaffold stub with its own issue — is the read that reports each one, so no implemented
/// read names them yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundResource {
    /// "The type of the resource, such as `AWS::DynamoDB::Table`"
    /// (`ScannedResource$ResourceType`).
    pub resource_type: String,
    /// The resource's physical id in the service that owns it — the value the stack recorded
    /// for it, which is what names the resource outside CloudFormation.
    pub physical_resource_id: String,
    /// The logical id the stack that manages this resource declares it under.
    pub logical_resource_id: String,
    /// The ARN of the stack that manages the resource, which is what
    /// `ScannedResource$ManagedByStack` reports the presence of ("if `true`, the resource is
    /// managed by a CloudFormation stack").
    pub stack_id: String,
}

/// The terminal status a scan is headed for, and the instant it is due.
#[derive(Debug, Clone)]
pub struct PendingScanOutcome {
    /// [`status`]: the terminal status this scan reaches.
    pub status: String,
    pub status_reason: Option<String>,
    /// `PercentageCompleted` as of that status.
    pub percentage_completed: f64,
    pub due_at: SystemTime,
}

/// ONE resource scan, carrying every member `ResourceScanSummary` reports and the Region
/// whose account holds it.
#[derive(Debug, Clone)]
pub struct StoredResourceScan {
    /// `ResourceScanId`: "the Amazon Resource Name (ARN) of the resource scan", in the
    /// documented `arn:${Partition}:cloudformation:${Region}:${Account}:resourceScan/${Id}`
    /// form [`resource_scan_arn`] builds.
    pub resource_scan_id: String,
    /// The Region this scan covered. A read answers for the Region its request was signed
    /// for, the way every other read of this service does.
    pub region: String,
    /// `ScanType`: [`scan_type`] — "the scan type that has been completed", which is also
    /// what `ScanTypeFilter` selects on.
    pub scan_type: String,
    /// `Status`: [`status`].
    pub status: String,
    /// `StatusReason`: "the reason for the resource scan status, providing more information
    /// if a failure happened".
    pub status_reason: Option<String>,
    /// `StartTime`: "the time that the resource scan was started", which is also the order
    /// the summaries are reported in — newest to oldest.
    pub start_time: SystemTime,
    /// `EndTime`: "the time that the resource scan was finished". `None` while the scan is
    /// still running, since it has not finished.
    pub end_time: Option<SystemTime>,
    /// `PercentageCompleted`: "the percentage of the resource scan that has been
    /// completed".
    pub percentage_completed: Option<f64>,
    /// The terminal status the clock owes this scan, until
    /// [`crate::state::ComAmazonawsCloudformationCloudformationState::settle`] materialises
    /// it.
    pub pending: Option<PendingScanOutcome>,
    /// `StartResourceScanInput$ClientRequestToken`: "A unique identifier for this
    /// StartResourceScan request. Specify this token if you plan to retry requests so that
    /// CloudFormation knows that you're not attempting to start a new resource scan" — so a
    /// retry under the token of the scan still running addresses this record instead of
    /// starting a second one.
    pub client_request_token: Option<String>,
    /// The `ScanFilters` the call supplied, one entry per filter carrying that filter's
    /// `Types`, which `DescribeResourceScan` reports back as "the scan filters that were
    /// used".
    pub scan_filters: Vec<Vec<String>>,
    /// How many resources the scan READ from the account and Region: every resource it looked
    /// at, before its filters decided which ones it lists.
    pub resources_read: usize,
    /// The resources the scan LISTED: the ones it read that its filters retained.
    pub resources: Vec<FoundResource>,
}

impl StoredResourceScan {
    /// A scan as the call that starts it first stores it: in flight, nothing scanned yet,
    /// and its completion due one [`SCAN_DELAY`] from that call.
    pub fn starting(
        resource_scan_id: String,
        region: String,
        scan_type: String,
        now: SystemTime,
    ) -> Self {
        Self {
            resource_scan_id,
            region,
            scan_type,
            status: status::IN_PROGRESS.to_string(),
            status_reason: None,
            start_time: now,
            end_time: None,
            percentage_completed: Some(PERCENTAGE_STARTED),
            pending: Some(PendingScanOutcome {
                status: status::COMPLETE.to_string(),
                status_reason: None,
                percentage_completed: PERCENTAGE_COMPLETE,
                due_at: now + SCAN_DELAY,
            }),
            client_request_token: None,
            scan_filters: Vec::new(),
            resources_read: 0,
            resources: Vec::new(),
        }
    }

    /// Materialise the status transition the clock has already made due, if any.
    ///
    /// Only ever applies a transition whose due instant has passed, so it reports what was
    /// owed at `now` even if nobody had called. The scan FINISHING is what stamps
    /// `EndTime`, so that instant is the one that was due rather than now.
    pub fn settle(&mut self, now: SystemTime) {
        if self.pending.as_ref().is_none_or(|owed| owed.due_at > now) {
            return;
        }
        let owed = self.pending.take().expect("checked just above");
        self.status = owed.status;
        self.status_reason = owed.status_reason;
        self.percentage_completed = Some(owed.percentage_completed);
        self.end_time = Some(owed.due_at);
    }

    /// Whether this record is one the `ScanTypeFilter` selects: "the scan type that you want
    /// to get summary information about".
    pub fn is_scan_type(&self, scan_type: &str) -> bool {
        self.scan_type == scan_type
    }

    /// Whether this scan is still running — the set the one-scan-at-a-time refusal meters.
    pub fn is_in_flight(&self) -> bool {
        self.status == status::IN_PROGRESS
    }

    /// Whether `resource_scan_id` addresses this record in `region`: a scan is addressed by
    /// its own ARN, and nothing outside the request's own Region is addressable — the way
    /// every other read of this service is Regional
    /// ([`crate::state::ComAmazonawsCloudformationCloudformationState::resolve`]).
    pub fn is_addressed_by(&self, region: &str, resource_scan_id: &str) -> bool {
        self.region == region && self.resource_scan_id == resource_scan_id
    }

    /// "The number of resources that were listed": the rows the scan's filters retained.
    pub fn resources_scanned(&self) -> usize {
        self.resources.len()
    }

    /// "The list of resource types for the specified scan": the distinct types among the
    /// resources the scan listed, in sorted order so the read is deterministic.
    pub fn resource_types(&self) -> Vec<String> {
        let mut types: Vec<String> = self
            .resources
            .iter()
            .map(|resource| resource.resource_type.clone())
            .collect();
        types.sort();
        types.dedup();
        types
    }

    /// Whether the counts the scan took are readable: "only available for scans with a
    /// `Status` set to `COMPLETE`, `EXPIRED`, or `FAILED`" — so a scan still running reports
    /// no counts rather than partial ones.
    pub fn counts_are_available(&self) -> bool {
        !self.is_in_flight()
    }

    /// Whether the types the scan found are readable: "resource types are only available for
    /// scans with a `Status` set to `COMPLETE` or `FAILED`", which leaves out a scan still
    /// running and one whose results have expired.
    pub fn types_are_available(&self) -> bool {
        matches!(self.status.as_str(), status::COMPLETE | status::FAILED)
    }
}

/// Whether a scan given `filters` lists a resource of `resource_type`.
///
/// A scan with no filter, or a filter that names no type, covers every resource it read —
/// `ScanFilter` is "a filter that is used to specify which resource types to scan", so naming
/// none narrows nothing. Otherwise the resource is listed when any named type matches it.
pub fn filters_admit(filters: &[Vec<String>], resource_type: &str) -> bool {
    let mut named_any = false;
    for types in filters {
        for filter in types {
            named_any = true;
            if type_filter_matches(filter, resource_type) {
                return true;
            }
        }
    }
    !named_any
}

/// Whether one `ScanFilter$Types` entry matches `resource_type`: a literal type matches
/// itself, and one ending in the documented wildcard matches every type under the prefix
/// before it ("to scan all resource types within a service, you can use a wildcard … for
/// example, `AWS::S3::*`").
pub fn type_filter_matches(filter: &str, resource_type: &str) -> bool {
    match filter.strip_suffix(TYPE_FILTER_WILDCARD) {
        Some(prefix) => resource_type.starts_with(prefix),
        None => filter == resource_type,
    }
}

/// Whether one `ScanFilter$Types` entry is one the service accepts: the wildcard may be
/// placed "at only the end of the string", so an asterisk anywhere else makes the filter
/// invalid.
pub fn is_valid_type_filter(filter: &str) -> bool {
    let body = filter.strip_suffix(TYPE_FILTER_WILDCARD).unwrap_or(filter);
    !body.contains(TYPE_FILTER_WILDCARD)
}

/// The scan type a call that supplied `filters` runs as: a scan given resource-type filters
/// covered PART of the account, and one given none covered all of it.
pub fn scan_type_of(filters: &[Vec<String>]) -> &'static str {
    if filters.iter().any(|types| !types.is_empty()) {
        scan_type::PARTIAL
    } else {
        scan_type::FULL
    }
}

/// The message answered for a `ScanFilters` entry that places the wildcard anywhere but at
/// the end of the string, built from the value the request supplied.
///
/// The rule is the model's own (`ScanFilter$Types`); the sentence that states it back to the
/// caller is this emulator's, since neither the model nor a recorded refusal of this request
/// carries the service's own wording for it.
pub fn invalid_type_filter(filter: &str) -> String {
    format!(
        "Invalid resource type filter [{filter}] in ScanFilters: an asterisk can only be \
         placed at the end of the string"
    )
}

/// The message answered when a request names a resource scan this account and Region holds no
/// record of, built from what the request named.
///
/// Shaped like the other "cannot be found" messages of this service
/// ([`crate::state::generated_template::does_not_exist`]): no recorded refusal of this request
/// is available to take the service's own wording from.
pub fn does_not_exist(resource_scan_id: &str) -> String {
    format!("Resource scan {resource_scan_id} cannot be found")
}

/// The message answered when a scan of this account and Region is already running, which is
/// the sentence the modelled `ResourceScanInProgressException` documents.
pub const SCAN_IN_PROGRESS: &str =
    "A resource scan is currently in progress. Only one can be run at a time for an account \
     in a Region.";

/// A scan's ARN from its id: `ResourceScanId` is "the Amazon Resource Name (ARN) of the
/// resource scan. The format is
/// `arn:${Partition}:cloudformation:${Region}:${Account}:resourceScan/${Id}`."
///
/// The account segment comes from the emulator-wide `emulator_arn` helper, like every other
/// ARN this service hands out ([`crate::state::new_generated_template_id`]).
pub fn resource_scan_arn(region: &str, id: &str) -> String {
    ::emulator_server::emulator_arn(
        crate::state::ARN_NAMESPACE,
        region,
        &format!("resourceScan/{id}"),
    )
}
