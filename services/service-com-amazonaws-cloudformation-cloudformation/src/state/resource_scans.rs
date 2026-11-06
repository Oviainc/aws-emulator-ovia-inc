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
//! The resources of this account and Region, wherever they are held: every resource of every
//! live stack of that Region
//! ([`crate::state::ComAmazonawsCloudformationCloudformationState::scannable_resources`]) and
//! every resource the services that OWN them report for that Region
//! ([`crate::state::resource_inventory`]), the two merged into one row per resource
//! ([`crate::state::resource_inventory::merged`]).
//!
//! A row therefore records the stack that manages the resource only where one does, which is
//! what `ScannedResource`'s `ManagedByStack` reports of it: a resource created straight
//! through its owning service's API is in no stack and is reported as the unmanaged resource
//! it is — the resources a scan exists to DISCOVER. The stack recorded on a managed row is
//! also the record of which resources belong TOGETHER, which is what
//! [`StoredResourceScan::related_resources`] answers `ListResourceScanRelatedResources` from.
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

use std::collections::{BTreeMap, BTreeSet};
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

/// Whether a record is one a request naming `wanted` as its `ScanTypeFilter` reports: "the
/// scan type that you want to get summary information about" selects the scans of THAT type,
/// and a request naming no scan type selects over every type — a scan the account started is
/// reported by the listing whatever the call that started it covered, so the same scan is
/// visible from the item read and from the listing.
pub fn selected_by_scan_type(stored_scan_type: &str, wanted: Option<&str>) -> bool {
    wanted.is_none_or(|wanted| wanted == stored_scan_type)
}

/// How many summaries a response carries when the request names no `MaxResults`:
/// "The default value is 10."
pub const DEFAULT_MAX_RESULTS: usize = 10;

/// The most summaries one response carries, whatever `MaxResults` asks for:
/// "The maximum value is 100."
pub const MAX_RESULTS_CEILING: usize = 100;

/// How many related resources one response carries when the request names no `MaxResults`:
/// "By default the ListResourceScanRelatedResources API action will return up to 100 results
/// in each response. The maximum value is 100."
pub const DEFAULT_RELATED_MAX_RESULTS: usize = 100;

/// How many scanned resources one response carries when the request names no `MaxResults`:
/// "By default the ListResourceScanResources API action will return at most 100 results in
/// each response. The maximum value is 100."
pub const DEFAULT_SCANNED_MAX_RESULTS: usize = 100;

/// How many resources one `ListResourceScanRelatedResources` request may ask about: "Up to 100
/// resources can be provided" (`ListResourceScanRelatedResourcesInput$Resources`), a bound the
/// model carries no `@length` for.
pub const MAX_REQUESTED_RESOURCES: usize = 100;

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

/// How many items one response of a resource-scan read carries, from the `MaxResults` the
/// request named and that read's own documented default.
///
/// `MaxResults` is what "the number of available results exceeds" before a `NextToken` is
/// issued, and each of these reads publishes a default and the same ceiling
/// ([`MAX_RESULTS_CEILING`]) in its member's documentation. The model puts no range on
/// `ResourceScannerMaxResults`, so the published ceiling is the whole of the rule for a maximum
/// ABOVE it — it is held to the ceiling rather than refused — and a value that is no maximum at
/// all (zero, or a negative one) names no page a response could carry, so the read's own
/// documented default serves it.
///
/// One spelling of that rule for every read of a scan, since they differ only in which default
/// they fall back to; the ceiling is applied here as well as by
/// [`crate::state::paginate_at_most`], so a later change to the service-wide page size cannot
/// quietly move these operations' documented maximum.
pub fn page_size(max_results: Option<i32>, default: usize) -> usize {
    max_results
        .and_then(|asked_for| usize::try_from(asked_for).ok())
        .filter(|asked_for| *asked_for >= 1)
        .unwrap_or(default)
        .min(MAX_RESULTS_CEILING)
}

/// The wildcard a resource-type filter may end with: "To scan all resource types within a
/// service, you can use a wildcard, represented by an asterisk (`*`). You can place an
/// asterisk at only the end of the string, for example, `AWS::S3::*`"
/// (`ScanFilter$Types`).
pub const TYPE_FILTER_WILDCARD: char = '*';

/// One resource a scan FOUND: what identifies it, and the stack that manages it.
///
/// `DescribeResourceScan` reports the COUNT of these rows and the distinct types among them;
/// the per-resource members are what a row IS, and `ListResourceScanRelatedResources` is the
/// read that reports each one as a `ScannedResource`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundResource {
    /// "The type of the resource, such as `AWS::DynamoDB::Table`"
    /// (`ScannedResource$ResourceType`).
    pub resource_type: String,
    /// The resource's physical id in the service that owns it — the value a stack recorded
    /// for it, or the one that service reports it under, which is what names the resource
    /// outside CloudFormation.
    pub physical_resource_id: String,
    /// The logical id the stack that manages this resource declares it under; EMPTY for a
    /// resource no stack manages, which declares no logical id for it.
    pub logical_resource_id: String,
    /// The ARN of the stack that manages the resource, which is what
    /// `ScannedResource$ManagedByStack` reports the presence of ("if `true`, the resource is
    /// managed by a CloudFormation stack") — EMPTY for a resource CloudFormation does not
    /// manage.
    pub stack_id: String,
    /// `ScannedResource$ResourceIdentifier`: "the key is the name of one of the primary
    /// identifiers for the resource … the value is the value of that primary identifier" —
    /// which is also what a request ADDRESSES one of these rows by
    /// (`ScannedResourceIdentifier`). Derived from the resource the scan read
    /// ([`primary_identifier`]) when the row is taken, so a read reports it rather than
    /// recomputing it.
    pub resource_identifier: BTreeMap<String, String>,
    /// The tags the resource carries in the service that owns it: for a resource a stack
    /// deployed, the ones its own declaration asked for plus the stack-level tags
    /// CloudFormation propagates to the resources it creates and the ones it records itself
    /// ([`crate::state::provisioning::StackProvenance::tags_for`]) — the set a request's
    /// `TagKey` and `TagValue` select on. Taken from the resource when the row is taken, like
    /// every other member of a row, so a read reports it rather than recomputing it.
    pub tags: BTreeMap<String, String>,
}

impl FoundResource {
    /// Whether `resource_type` and `identifier` ADDRESS this resource: a
    /// `ScannedResourceIdentifier` names the type of the resource it identifies and the
    /// primary-identifier pairs that pick it out, so the type must be this row's and every
    /// pair the caller supplied must be one this row carries. An identifier naming no pair at
    /// all picks out nothing rather than everything.
    pub fn is_addressed_by(
        &self,
        resource_type: &str,
        identifier: &BTreeMap<String, String>,
    ) -> bool {
        !identifier.is_empty()
            && self.resource_type == resource_type
            && identifier
                .iter()
                .all(|(key, value)| self.resource_identifier.get(key) == Some(value))
    }

    /// `ScannedResource$ManagedByStack`: "if `true`, the resource is managed by a
    /// CloudFormation stack" — which is whether this row records the stack that manages it.
    pub fn is_managed_by_stack(&self) -> bool {
        !self.stack_id.is_empty()
    }

    /// Whether this row carries `resource_identifier` as one of its primary identifiers:
    /// `ListResourceScanResourcesInput$ResourceIdentifier` selects the resources that "have
    /// the specified resource identifier (or one of them in the case where the resource has
    /// multiple identifiers)", so it is matched against the VALUES of the row's identifier
    /// rather than against the member names they are the values of.
    pub fn has_identifier(&self, resource_identifier: &str) -> bool {
        self.resource_identifier
            .values()
            .any(|held| held == resource_identifier)
    }

    /// Whether this row's type begins with `prefix`:
    /// `ListResourceScanResourcesInput$ResourceTypePrefix` selects the resources "of any of
    /// the resource types with the specified prefix".
    pub fn has_type_prefix(&self, prefix: &str) -> bool {
        self.resource_type.starts_with(prefix)
    }

    /// Whether this row carries a tag whose key is `tag_key`:
    /// `ListResourceScanResourcesInput$TagKey` selects the resources that "have a matching tag
    /// key".
    pub fn has_tag_key(&self, tag_key: &str) -> bool {
        self.tags.contains_key(tag_key)
    }

    /// Whether this row carries a tag whose value is `tag_value`:
    /// `ListResourceScanResourcesInput$TagValue` selects the resources that "have a matching
    /// tag value".
    pub fn has_tag_value(&self, tag_value: &str) -> bool {
        self.tags.values().any(|held| held == tag_value)
    }
}

/// The filters one `ListResourceScanResources` request narrows a scan's resources by: "the
/// results can be filtered by resource identifier, resource type prefix, tag key, and tag
/// value".
///
/// A filter the request left out narrows nothing, and a resource is reported only when EVERY
/// filter it did name admits it — "only resources that match all specified filters are
/// returned".
#[derive(Debug, Clone, Default)]
pub struct ResourceFilters<'a> {
    /// `ResourceIdentifier` ([`FoundResource::has_identifier`]).
    pub resource_identifier: Option<&'a str>,
    /// `ResourceTypePrefix` ([`FoundResource::has_type_prefix`]).
    pub resource_type_prefix: Option<&'a str>,
    /// `TagKey` ([`FoundResource::has_tag_key`]).
    pub tag_key: Option<&'a str>,
    /// `TagValue` ([`FoundResource::has_tag_value`]).
    pub tag_value: Option<&'a str>,
}

impl ResourceFilters<'_> {
    /// Whether every filter this request named admits `found`.
    pub fn admit(&self, found: &FoundResource) -> bool {
        self.resource_identifier
            .is_none_or(|identifier| found.has_identifier(identifier))
            && self
                .resource_type_prefix
                .is_none_or(|prefix| found.has_type_prefix(prefix))
            && self.tag_key.is_none_or(|key| found.has_tag_key(key))
            && self.tag_value.is_none_or(|value| found.has_tag_value(value))
    }
}

/// One resource a scan found, as the modelled `ScannedResource`: "a scanned resource returned
/// by `ListResourceScanResources` or `ListResourceScanRelatedResources`", every member read
/// off the row the scan took.
///
/// Both reads report the same shape from the same rows, so both build it here.
pub fn scanned_resource(found: &FoundResource) -> crate::ops::types::ScannedResource {
    crate::ops::types::ScannedResource {
        resource_type: Some(found.resource_type.clone()),
        resource_identifier: Some(
            found
                .resource_identifier
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect(),
        ),
        // "The response indicates whether each returned resource is already managed by
        // CloudFormation": whether the row records the stack that manages it.
        managed_by_stack: Some(found.is_managed_by_stack()),
    }
}

/// The tags a resource a scan read carries in the service that owns it.
///
/// CloudFormation does not only create a stack's resources, it TAGS them: the stack-level
/// tags are "propagated to the resources that CloudFormation creates in the stack" and
/// CloudFormation records its own beside them, while the resource's declaration is the more
/// specific statement about that one resource — which is exactly the set a deployment sends
/// the owning service ([`crate::state::provisioner::tags_of`]), so a scan reads the resource's
/// tags through the same rule rather than through a second account of it.
pub fn resource_tags(
    resource: &crate::state::StoredResource,
    provenance: &crate::state::provisioning::StackProvenance,
) -> BTreeMap<String, String> {
    crate::state::provisioner::tags_of(
        &resource.properties,
        &provenance.tags_for(&resource.logical_id),
    )
    .into_iter()
    .collect()
}

/// The primary identifier of one resource a scan read, as `ScannedResource$ResourceIdentifier`
/// reports it: "the key is the name of one of the primary identifiers for the resource …  the
/// value is the value of that primary identifier".
///
/// The VALUE is the resource's physical id, which is what identifies it in the service that
/// owns it. The KEY is the member that id is the value of, taken from what this service already
/// knows about the type, most specific first:
///
/// 1. the PUBLISHED return value that is the physical id — a type whose `Ref` returns an ARN or
///    a URL is identified by that member (`AWS::SNS::Topic`'s `TopicArn`, `AWS::SQS::Queue`'s
///    `QueueUrl`), which is the published attribute
///    ([`crate::state::template::published_attribute_names`]) carrying that same value;
/// 2. otherwise the property the declaration NAMED the resource under
///    ([`crate::state::template::name_property`]), since a physical id that is the resource's
///    name is the value of that property;
/// 3. otherwise the name property CloudFormation's convention gives the type
///    ([`crate::state::template::name_property_candidates`]), for a resource whose name the
///    template left CloudFormation to generate.
///
/// Only the PUBLISHED attribute names are considered in the first step: a resource's stored
/// attribute map also carries the aliases a template may read an attribute under, and an alias
/// is not a name the owning service identifies the resource by.
///
/// The third step always answers, since the conventional candidates are built from the type's
/// own name, so every row carries one pair — never an empty identifier, which would be a row no
/// request could address.
pub fn primary_identifier(resource: &crate::state::StoredResource) -> BTreeMap<String, String> {
    let published = crate::state::template::published_attribute_names(&resource.resource_type)
        .iter()
        .find(|name| {
            resource.attributes.get(**name) == Some(&resource.physical_resource_id)
        })
        .map(|name| (*name).to_string());
    let key = published
        .or_else(|| {
            crate::state::template::name_property(&resource.resource_type, &resource.properties)
        })
        .or_else(|| {
            crate::state::template::name_property_candidates(&resource.resource_type)
                .into_iter()
                .next()
        });
    key.into_iter()
        .map(|key| (key, resource.physical_resource_id.clone()))
        .collect()
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
    /// to get summary information about", or every record where the request names none
    /// ([`selected_by_scan_type`]).
    pub fn is_scan_type(&self, scan_type: Option<&str>) -> bool {
        selected_by_scan_type(&self.scan_type, scan_type)
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

    /// The scan's count of listed resources BROKEN DOWN BY TYPE: how many of the rows it
    /// listed each distinct resource type among them accounts for.
    ///
    /// The same rows [`Self::resources_scanned`] counts and [`Self::resource_types`] takes
    /// the distinct types of, grouped instead of totalled — so the breakdown and the two
    /// totals cannot disagree about one scan — keyed by type name, which orders it the way
    /// the type list is ordered and makes the read deterministic.
    pub fn resources_scanned_by_type(&self) -> BTreeMap<&str, usize> {
        let mut by_type: BTreeMap<&str, usize> = BTreeMap::new();
        for resource in &self.resources {
            *by_type
                .entry(resource.resource_type.as_str())
                .or_insert(0) += 1;
        }
        by_type
    }

    /// Whether the types the scan found are readable: "resource types are only available for
    /// scans with a `Status` set to `COMPLETE` or `FAILED`", which leaves out a scan still
    /// running and one whose results have expired.
    pub fn types_are_available(&self) -> bool {
        matches!(self.status.as_str(), status::COMPLETE | status::FAILED)
    }

    /// The resources this scan LISTED that `filters` admits, in the order the scan found
    /// them: "list of up to MaxResults resources in the specified resource scan that match all
    /// of the specified filters".
    pub fn filtered_resources(&self, filters: &ResourceFilters<'_>) -> Vec<&FoundResource> {
        self.resources
            .iter()
            .filter(|found| filters.admit(found))
            .collect()
    }

    /// The resources this scan listed that are RELATED to the ones `requested` addresses,
    /// each `(resource type, primary identifier)` as a `ScannedResourceIdentifier` names one.
    ///
    /// A resource's related resources are the OTHER resources of the scan that the same
    /// CloudFormation stack manages: a stack is the record this service holds of which
    /// resources belong together, so the resources it deployed from one template are the ones
    /// related to each other. A requested resource this scan did not list contributes nothing,
    /// and a requested resource is never related to itself.
    ///
    /// The order is the order the scan found the rows in, so paging through the result walks
    /// it once; a row is reported once however many of the requested resources it is related
    /// to.
    pub fn related_resources(
        &self,
        requested: &[(String, BTreeMap<String, String>)],
    ) -> Vec<&FoundResource> {
        let addressed: Vec<bool> = self
            .resources
            .iter()
            .map(|found| {
                requested.iter().any(|(resource_type, identifier)| {
                    found.is_addressed_by(resource_type, identifier)
                })
            })
            .collect();
        let stacks: BTreeSet<&str> = self
            .resources
            .iter()
            .zip(&addressed)
            .filter(|(_, addressed)| **addressed)
            .map(|(found, _)| found.stack_id.as_str())
            .filter(|stack_id| !stack_id.is_empty())
            .collect();
        self.resources
            .iter()
            .zip(&addressed)
            .filter(|(found, addressed)| {
                !**addressed && stacks.contains(found.stack_id.as_str())
            })
            .map(|(found, _)| found)
            .collect()
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

/// The shape a `ResourceScanId` must have, as CloudFormation's own input validation states
/// it back to the caller.
///
/// `ResourceScanId` is "the Amazon Resource Name (ARN) of the resource scan. The format is
/// `arn:${Partition}:cloudformation:${Region}:${Account}:resourceScan/${Id}`", and the
/// service holds the member to exactly that form before it looks anything up. The model
/// carries no `@pattern` for the shape, so the handler is what enforces it — the same
/// position [`crate::state::STACK_NAME_PATTERN`] is in — and the pattern is reported in the
/// refusal verbatim, so it is spelled here once and nowhere else.
pub const RESOURCE_SCAN_ID_PATTERN: &str =
    "arn:[a-z0-9-]+:cloudformation:[a-z0-9-]+:[0-9]{12}:resourceScan/[a-z0-9-]+";

/// Whether every character of `segment` is one `[a-z0-9-]` admits, and there is at least
/// one — the character class [`RESOURCE_SCAN_ID_PATTERN`] spells three of its segments with.
fn is_lower_alphanumeric_or_hyphen(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Whether `resource_scan_id` satisfies [`RESOURCE_SCAN_ID_PATTERN`], matched over the whole
/// value the way the service's own anchored pattern match is.
///
/// Spelled out segment by segment rather than compiled, since the pattern is fixed: the
/// partition and the Region are `[a-z0-9-]+`, the service segment is this service's own ARN
/// namespace, the account is twelve digits, and the resource segment is `resourceScan/`
/// followed by the scan's id.
pub fn is_resource_scan_id(resource_scan_id: &str) -> bool {
    let mut segments = resource_scan_id.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(region), Some(account), Some(resource)) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return false;
    };
    let Some(id) = resource.strip_prefix("resourceScan/") else {
        return false;
    };
    is_lower_alphanumeric_or_hyphen(partition)
        && service == crate::state::ARN_NAMESPACE
        && is_lower_alphanumeric_or_hyphen(region)
        && account.len() == 12
        && account.chars().all(|c| c.is_ascii_digit())
        && is_lower_alphanumeric_or_hyphen(id)
}

/// `Err(message)` for a `ResourceScanId` outside [`RESOURCE_SCAN_ID_PATTERN`], in the
/// sentence CloudFormation's input validation words that refusal with.
///
/// A value of the wrong SHAPE is an input-constraint violation, not a missing resource: it
/// addresses no scan that could ever exist, so every read of a scan checks it ahead of the
/// state lookup and answers `ValidationError` rather than [`NOT_FOUND`] — which is what
/// lets a caller tell a typo in the id from a scan that is not there.
///
/// The rejected value is echoed so the caller can see what was refused, bounded at
/// [`MAX_ECHOED_VALUE`]: the member carries no modelled `@length`, so one request could
/// otherwise make the refusal as long as the value it sent.
pub fn validate_resource_scan_id(resource_scan_id: &str) -> Result<(), String> {
    if is_resource_scan_id(resource_scan_id) {
        return Ok(());
    }
    Err(format!(
        "1 validation error detected: Value '{}' at 'resourceScanId' failed to satisfy \
         constraint: Member must satisfy regular expression pattern: \
         {RESOURCE_SCAN_ID_PATTERN}",
        echoed(resource_scan_id)
    ))
}

/// Ceiling on the rejected value as a refusal echoes it, and the elision past it: the
/// framework's own, so a refusal this module words and one the boundary validator words
/// quote a caller's value the same way.
const MAX_ECHOED_VALUE: usize = 256;

/// `value` as a refusal may quote it: itself when short, a character-boundary-safe prefix
/// with the elision when not.
///
/// Rendering only — the CHECK above runs on the whole value, since eliding first would
/// compare text the caller never sent.
fn echoed(value: &str) -> ::std::borrow::Cow<'_, str> {
    if value.len() <= MAX_ECHOED_VALUE {
        return ::std::borrow::Cow::Borrowed(value);
    }
    let mut end = MAX_ECHOED_VALUE;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    ::std::borrow::Cow::Owned(format!("{}... [{} bytes]", &value[..end], value.len()))
}

/// The `ResourceScanNotFoundException` message CloudFormation answers when a request names a
/// resource scan this account and Region holds no record of — an ARN no scan was started
/// under, one another Region holds, or one this store no longer retains.
///
/// This is the sentence the service's OWN refusal of such a request carries, which is what
/// replaced the stand-in wording this module previously built from the id the request named:
/// the real refusal names nothing of the request, so there is no member to interpolate — a
/// FIXED sentence, the way [`crate::state::HOOK_RESULT_NOT_FOUND`] is. It belongs to the error
/// SHAPE rather than to one operation, so every read of a scan answers it.
pub const NOT_FOUND: &str = "There is no ResourceScan found for the ResourceScanId.";

/// The message answered when a request asks about more resources than one call may carry,
/// built from how many it supplied and the documented bound.
///
/// The rule is the model's own (`ListResourceScanRelatedResourcesInput$Resources`: "Up to 100
/// resources can be provided"); the sentence that states it back to the caller is this
/// emulator's, since neither the model nor a recorded refusal of this request carries the
/// service's own wording for it — the same position [`invalid_type_filter`] is in.
pub fn too_many_requested_resources(requested: usize) -> String {
    format!(
        "Resources can carry at most {MAX_REQUESTED_RESOURCES} resources, but {requested} \
         were provided"
    )
}

/// The message answered when a scan of this account and Region is already running, which is
/// the sentence the modelled `ResourceScanInProgressException` documents. The refusal of a
/// request to START another scan: what it states is the exclusivity.
pub const SCAN_IN_PROGRESS: &str =
    "A resource scan is currently in progress. Only one can be run at a time for an account \
     in a Region.";

/// The message answered a request that operates ON a scan that has not finished — a read of
/// the result a running scan has not produced yet.
///
/// The same modelled `ResourceScanInProgressException`, but a DIFFERENT sentence from
/// [`SCAN_IN_PROGRESS`]: this one tells the caller to wait for THAT scan and where to watch
/// it, rather than stating the one-scan-at-a-time rule a start is refused under. Entirely
/// fixed text, naming nothing of the request.
pub const OPERATION_WHILE_SCANNING: &str =
    "The ResourceScan is in progress. Please wait for it to finish before performing any \
     further operations on it. Describe ResourceScans to see the status of the in-progress \
     ResourceScan.";

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
