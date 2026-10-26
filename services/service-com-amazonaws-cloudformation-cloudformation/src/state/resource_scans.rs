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
//! the ONE way a record enters the store — so that operation (still a scaffold stub, with
//! its own issue) and the suites that cover the reads over records a bound's worth of wire
//! calls cannot create both write through it rather than reaching into the vector. An
//! account whose resources have never been scanned is exactly what this store holds for
//! now, and that is what the read reports: the empty list, for every filter a request can
//! name.

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
}

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
