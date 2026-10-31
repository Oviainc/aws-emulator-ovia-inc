// SPDX-License-Identifier: CC0-1.0

//! AMI usage reports: what one is stored as, the words and bounds its members take, and the
//! two moves the clock owes one.
//!
//! "Creates a report that shows how your image is used across other Amazon Web Services
//! accounts" (`CreateImageUsageReport`): a report is a RESOURCE of the account, held in
//! [`ComAmazonawsEc2Amazonec2State::image_usage_reports`] as the modelled
//! [`ImageUsageReport`] shape `DescribeImageUsageReports` reports, so every member a writer
//! sets round-trips without a field-by-field copy that can silently drop one.
//!
//! Two instants a report carries are not members the caller chooses:
//!
//! * "The date and time when the report was created" (`ImageUsageReport$CreationTime`) is
//!   read off the clock at the create and never again.
//! * "The date and time when Amazon EC2 will delete the report (30 days after the report was
//!   created)" (`ImageUsageReport$ExpirationTime`) is that instant plus
//!   [`IMAGE_USAGE_REPORT_RETENTION`], and the DELETION it names is owed to the clock:
//!   [`ComAmazonawsEc2Amazonec2State::expire_due_image_usage_reports`] applies it, the way
//!   the Recycle Bin's permanent deletions are applied, so a report past that instant is
//!   gone whether or not anybody called.
//!
//! A report AWS hands back is not readable yet — "`pending` - The report is being created and
//! not available to view" (`ImageUsageReport$State`) — so a created report is stored
//! [`IMAGE_USAGE_REPORT_STATE_PENDING`] and the clock owes it
//! [`IMAGE_USAGE_REPORT_STATE_AVAILABLE`] one [`TRANSITION_DELAY`] later, the span every other
//! non-terminal state of this store moves on. The due instant sits in
//! [`ComAmazonawsEc2Amazonec2State::image_usage_report_available_at`], beside the report
//! rather than on it, the way a `pending` VPC's does, and
//! [`ComAmazonawsEc2Amazonec2State::materialize_due_image_usage_reports`] applies it — never
//! slept for, so a caller reading straight after the create still sees `pending`.
//!
//! A SUBMODULE of [`crate::state`], for the same reason
//! [`super::application_status_checks`] is one: this surface grows as the sibling report
//! operations are implemented, and it grows here rather than in the parts of `state.rs` the
//! other resource kinds own. `pub use` keeps every name reachable as `crate::state::<name>`,
//! which is where each handler names it.

use std::time::{Duration, Instant, SystemTime};

use crate::ops::types::{ImageUsageReport, ImageUsageResourceType, Tag};
use crate::scaffolded::Scaffolded;

use super::{
    ComAmazonawsEc2Amazonec2State, IMAGE_USAGE_REPORT_ID_PREFIX, TRANSITION_DELAY, same_tags,
    system_time_to_date_time, tag_list_bytes,
};

/// "`pending` - The report is being created and not available to view"
/// (`ImageUsageReport$State`). `ImageUsageReportState` is a plain string in the model
/// (`smithy_query(query="shape",shape="ImageUsageReportState")`), so the catalog is named here
/// from that member's documentation.
pub const IMAGE_USAGE_REPORT_STATE_PENDING: &str = "pending";

/// "`available` - The report is available to view" (`ImageUsageReport$State`).
pub const IMAGE_USAGE_REPORT_STATE_AVAILABLE: &str = "available";

/// "`error` - The report could not be created" (`ImageUsageReport$State`). Nothing in this
/// slice produces it — a create either stores a report or is refused — so it is named for the
/// readers that compare against the documented catalog.
pub const IMAGE_USAGE_REPORT_STATE_ERROR: &str = "error";

/// The `TagSpecification$ResourceType` a create may name when it tags the report it creates:
/// "The ResourceType must be set to image-usage-report; any other value will cause the report
/// creation to fail" (`CreateImageUsageReportRequest$TagSpecifications`), which is also the
/// `image-usage-report` member of the modelled `ResourceType` catalog
/// (`crate::ops::types::RESOURCE_TYPE_VALUES`).
pub const IMAGE_USAGE_REPORT_TAGGABLE_RESOURCE: &str = "image-usage-report";

/// How long a report is kept: "the date and time when Amazon EC2 will delete the report (30
/// days after the report was created)" (`ImageUsageReport$ExpirationTime`) — a DOCUMENTED
/// duration, so it keeps its published value rather than a span this emulator chose.
pub const IMAGE_USAGE_REPORT_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The resource types a report may be created for: "Valid values: `ec2:Instance` |
/// `ec2:LaunchTemplate`" (`ImageUsageResourceTypeRequest$ResourceType`).
/// `ImageUsageResourceTypeName` is a plain string in the model
/// (`smithy_query(query="shape",shape="ImageUsageResourceTypeName")`), so the catalog is named
/// here from that member's documentation.
pub const IMAGE_USAGE_RESOURCE_TYPE_NAMES: &[&str] = &["ec2:Instance", "ec2:LaunchTemplate"];

/// The one resource type whose scope options are meaningful: "the options that affect the
/// scope of the report. Valid only when `ResourceType` is `ec2:LaunchTemplate`"
/// (`ImageUsageResourceTypeRequest$ResourceTypeOptions`).
pub const IMAGE_USAGE_RESOURCE_TYPE_LAUNCH_TEMPLATE: &str = "ec2:LaunchTemplate";

/// The one scope option a resource type may carry: "Valid value: `version-depth` - The number
/// of launch template versions to check"
/// (`ImageUsageResourceTypeOptionRequest$OptionName`).
pub const IMAGE_USAGE_VERSION_DEPTH_OPTION: &str = "version-depth";

/// The values that option takes: "Valid values: Integers between `1` and `10000`"
/// (`ImageUsageResourceTypeOptionRequest$OptionValues`). The member is a list of plain
/// strings with no modelled `@range`, so this is the only place those published bounds are
/// enforced.
pub const IMAGE_USAGE_VERSION_DEPTH_RANGE: (u32, u32) = (1, 10_000);

/// The value a launch-template resource type that names no option gets: "Default: `20`"
/// (`ImageUsageResourceTypeOptionRequest$OptionValues`), filled in at the create so the
/// stored report states the scope it was actually created with.
pub const DEFAULT_IMAGE_USAGE_VERSION_DEPTH: u32 = 20;

/// How many values one scope option carries: ONE.
///
/// "A value for the specified option" (`ImageUsageResourceTypeOptionRequest$OptionValues`) is
/// one value, so a request that gives several is refused rather than one of them being chosen
/// — the same reading the sibling `describe_image_references` operation applies to its own
/// shape's option. The list carries no modelled `@length`, so this is also what bounds the
/// values one stored report can retain.
pub const IMAGE_USAGE_RESOURCE_TYPE_OPTION_VALUES: usize = 1;

/// How many AMI usage reports this store holds before a create is refused.
///
/// CHOSEN: AWS publishes no quota for this resource, and a store that grows without bound is
/// not a store — the same reason [`super::MAX_IMAGES`] exists. Well above the number of
/// reports a test or a demo creates.
pub const MAX_IMAGE_USAGE_REPORTS: usize = 1_000;

/// Refuse a create that would take the account past [`MAX_IMAGE_USAGE_REPORTS`]: HTTP 400
/// [`super::RESOURCE_LIMIT_EXCEEDED`], the code this service answers for a ceiling with no
/// code of its own.
pub fn image_usage_report_limit_exceeded<E>(held: usize) -> Scaffolded<E> {
    super::resource_limit_exceeded(format!(
        "This emulator holds {held} of at most {MAX_IMAGE_USAGE_REPORTS} AMI usage reports, \
         so it cannot store another one"
    ))
}

/// The caller-chosen bytes one stored AMI usage report holds: its tags, the image id and
/// account ids it names, every resource type with its options, and the `ClientToken` the token
/// map keeps for it. The report id and the two instants are this module's to decide, so they
/// are fixed-size.
///
/// The ONE definition of that charge, the way
/// [`super::application_status_check_payload_bytes`] is for a check: a create asks it before
/// the write and charges exactly it, and
/// [`ComAmazonawsEc2Amazonec2State::remove_image_usage_report`] gives back the same number, so
/// a report that comes and goes leaves the budget where it found it.
pub fn image_usage_report_payload_bytes(
    report: &ImageUsageReport,
    client_token: Option<&str>,
) -> usize {
    let accounts: usize = report
        .account_ids
        .iter()
        .flatten()
        .map(|account_id| account_id.len())
        .sum();
    let resource_types: usize = report
        .resource_types
        .iter()
        .flatten()
        .map(resource_type_bytes)
        .sum();
    tag_list_bytes(report.tags.as_deref().unwrap_or_default())
        .saturating_add(report.image_id.as_deref().map_or(0, str::len))
        .saturating_add(accounts)
        .saturating_add(resource_types)
        .saturating_add(client_token.map_or(0, str::len))
}

/// The caller-chosen bytes one reported resource type holds: its name, and the name and values
/// of each scope option it carries.
fn resource_type_bytes(resource_type: &ImageUsageResourceType) -> usize {
    resource_type
        .resource_type_options
        .iter()
        .flatten()
        .fold(
            resource_type.resource_type.as_deref().map_or(0, str::len),
            |bytes, option| {
                let values: usize = option
                    .option_values
                    .iter()
                    .flatten()
                    .map(|value| value.len())
                    .sum();
                bytes
                    .saturating_add(option.option_name.as_deref().map_or(0, str::len))
                    .saturating_add(values)
            },
        )
}

/// Do two reports ask for the SAME report — every member a create takes from the request, and
/// nothing the service decides for itself (the id, the state and the instants)?
///
/// What an idempotency replay is judged on: "a unique, case-sensitive identifier that you
/// provide to ensure idempotency of the request"
/// (`CreateImageUsageReportRequest$ClientToken`) holds for a retry of the same request, so a
/// token reused for a different report names a report the caller did not ask for, which EC2
/// refuses instead. The account ids are a SET of accounts to include, compared as a sorted
/// list for the same reason [`same_tags`] sorts a tag list; the resource types are a sequence
/// the caller gave, so their order is part of what was asked. The modelled shapes carry no
/// `PartialEq`, which is why the comparison is spelled out here once rather than in each
/// operation that needs it.
pub fn same_image_usage_report_request(stored: &ImageUsageReport, asked: &ImageUsageReport) -> bool {
    let sorted_accounts = |report: &ImageUsageReport| -> Vec<String> {
        let mut accounts: Vec<String> = report.account_ids.iter().flatten().cloned().collect();
        accounts.sort();
        accounts
    };
    stored.image_id == asked.image_id
        && sorted_accounts(stored) == sorted_accounts(asked)
        && same_resource_types(
            stored.resource_types.as_deref().unwrap_or_default(),
            asked.resource_types.as_deref().unwrap_or_default(),
        )
        && same_tags(
            stored.tags.as_deref().unwrap_or_default(),
            asked.tags.as_deref().unwrap_or_default(),
        )
}

/// Whether two resource type lists name the same types, in the same order, with the same scope
/// options.
fn same_resource_types(stored: &[ImageUsageResourceType], asked: &[ImageUsageResourceType]) -> bool {
    stored.len() == asked.len()
        && stored.iter().zip(asked).all(|(stored, asked)| {
            let stored_options = stored.resource_type_options.as_deref().unwrap_or_default();
            let asked_options = asked.resource_type_options.as_deref().unwrap_or_default();
            stored.resource_type == asked.resource_type
                && stored_options.len() == asked_options.len()
                && stored_options
                    .iter()
                    .zip(asked_options)
                    .all(|(stored, asked)| {
                        stored.option_name == asked.option_name
                            && stored.option_values == asked.option_values
                    })
        })
}

/// What one AMI usage report is created from: the members the request chooses, every documented
/// default already filled in by the operation that took them.
#[derive(Debug, Clone, Default)]
pub struct ImageUsageReportSpec {
    /// "The ID of the image to report on" (`CreateImageUsageReportRequest$ImageId`).
    pub image_id: String,
    /// "The resource types to include in the report"
    /// (`CreateImageUsageReportRequest$ResourceTypes`), as the report reports them.
    pub resource_types: Vec<ImageUsageResourceType>,
    /// "The Amazon Web Services account IDs to include in the report. To include all
    /// accounts, omit this parameter" (`CreateImageUsageReportRequest$AccountIds`) — so an
    /// empty list is the report over every account.
    pub account_ids: Vec<String>,
    /// "The tags to apply to the report on creation"
    /// (`CreateImageUsageReportRequest$TagSpecifications`).
    pub tags: Vec<Tag>,
}

impl ComAmazonawsEc2Amazonec2State {
    /// Store one AMI usage report created at `created_at`, and answer the `ReportId` it was
    /// minted with.
    ///
    /// The report is stored [`IMAGE_USAGE_REPORT_STATE_PENDING`] with
    /// [`IMAGE_USAGE_REPORT_STATE_AVAILABLE`] owed to the clock one [`TRANSITION_DELAY`]
    /// later (see the module docs), its creation instant read off `created_at` and its
    /// expiration that instant plus [`IMAGE_USAGE_REPORT_RETENTION`]. The bytes the record
    /// retains are NOT charged here: the create asks
    /// [`image_usage_report_payload_bytes`] against the store's budget before it calls this,
    /// the way every other create of this crate does.
    pub fn insert_image_usage_report(
        &mut self,
        spec: ImageUsageReportSpec,
        created_at: SystemTime,
    ) -> String {
        let report_id = self.next_id(IMAGE_USAGE_REPORT_ID_PREFIX);
        let report = ImageUsageReport {
            image_id: Some(spec.image_id),
            report_id: Some(report_id.clone()),
            resource_types: Some(spec.resource_types),
            account_ids: Some(spec.account_ids),
            state: Some(IMAGE_USAGE_REPORT_STATE_PENDING.to_string()),
            // "Provides additional details when the report is in an `error` state"
            // (`ImageUsageReport$StateReason`): a report being created carries no such
            // detail.
            state_reason: None,
            creation_time: Some(system_time_to_date_time(created_at)),
            // A deletion instant the store can represent; a clock without one owes no
            // deletion, which [`Self::expire_due_image_usage_reports`] reads as "never
            // expires".
            expiration_time: created_at
                .checked_add(IMAGE_USAGE_REPORT_RETENTION)
                .map(system_time_to_date_time),
            tags: Some(spec.tags),
        };
        self.image_usage_report_available_at
            .insert(report_id.clone(), Instant::now() + TRANSITION_DELAY);
        self.image_usage_reports.insert(report_id.clone(), report);
        report_id
    }

    /// Take the AMI usage report `report_id` out of the store, and answer the report that was
    /// removed — `None` when the account holds none under that id.
    ///
    /// The ONE removal path: it drops the report, the move the clock owed it and every
    /// `ClientToken` that named it (so a token can never name a report that is gone), and
    /// gives the store's byte budget back exactly what
    /// [`image_usage_report_payload_bytes`] charged for them. `DeleteImageUsageReport` and
    /// the clock-owed expiry below both go through it, so no path can leave the budget
    /// charged for a report nothing holds.
    pub fn remove_image_usage_report(&mut self, report_id: &str) -> Option<ImageUsageReport> {
        let report = self.image_usage_reports.remove(report_id)?;
        self.image_usage_report_available_at.remove(report_id);
        let tokens: usize = self
            .image_usage_report_client_tokens
            .iter()
            .filter(|(_token, named)| named.as_str() == report_id)
            .map(|(token, _named)| token.len())
            .sum();
        self.image_usage_report_client_tokens
            .retain(|_token, named| named.as_str() != report_id);
        self.release_stored_bytes(
            image_usage_report_payload_bytes(&report, None).saturating_add(tokens),
        );
        Some(report)
    }

    /// Apply the `pending` -> `available` move every report whose due instant has passed is
    /// owed, as of `now`.
    ///
    /// Reached from [`Self::materialize_due_transitions`] and from the reads of this slice, so
    /// a reporting operation observes the move rather than causing it: it fell due at this
    /// instant whether or not anybody called.
    pub fn materialize_due_image_usage_reports(&mut self, now: Instant) {
        let reports = &mut self.image_usage_reports;
        self.image_usage_report_available_at
            .retain(|report_id, due_at| {
                if now < *due_at {
                    return true;
                }
                if let Some(report) = reports.get_mut(report_id) {
                    if report.state.as_deref() == Some(IMAGE_USAGE_REPORT_STATE_PENDING) {
                        report.state = Some(IMAGE_USAGE_REPORT_STATE_AVAILABLE.to_string());
                    }
                }
                // The move is applied once; nothing is owed on this report any more.
                false
            });
    }

    /// Delete every AMI usage report whose expiration has come due as of `now`.
    ///
    /// "The date and time when Amazon EC2 will delete the report (30 days after the report was
    /// created)" (`ImageUsageReport$ExpirationTime`): past that instant the report no longer
    /// exists, so a reporting operation may apply the deletion — it was owed at this instant
    /// whether or not anybody asked, so materialising it changes nothing a later caller would
    /// otherwise have seen. A report carrying no expiration time is never owed one.
    ///
    /// Instants are compared as `(seconds, nanoseconds)`, the way every other instant
    /// comparison in this crate does it: the modelled timestamp type carries no `Ord`, and
    /// comparing through `f64` seconds would lose precision the `MillisecondDateTime` the
    /// member targets keeps. Every deletion goes through
    /// [`Self::remove_image_usage_report`], so an expiry refunds what the create charged.
    pub fn expire_due_image_usage_reports(&mut self, now: SystemTime) {
        fn key(at: ::aws_smithy_types::DateTime) -> (i64, u32) {
            (at.secs(), at.subsec_nanos())
        }

        let now = key(system_time_to_date_time(now));
        let due: Vec<String> = self
            .image_usage_reports
            .iter()
            .filter(|(_report_id, report)| {
                report
                    .expiration_time
                    .map(key)
                    .is_some_and(|expires_at| now >= expires_at)
            })
            .map(|(report_id, _report)| report_id.clone())
            .collect();
        for report_id in due {
            self.remove_image_usage_report(&report_id);
        }
    }
}
