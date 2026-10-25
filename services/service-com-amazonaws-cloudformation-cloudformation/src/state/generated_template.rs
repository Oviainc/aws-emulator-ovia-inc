// SPDX-License-Identifier: CC0-1.0

//! GENERATED TEMPLATES: the templates `CreateGeneratedTemplate` generates from resources a
//! stack already manages, and the generation each one's status reports the progress of.
//!
//! A generated template is a record of its own, not a stack's: `CreateGeneratedTemplate`
//! "creates a template from existing resources" and hands back the template's id, and
//! "you can check the status of the template generation using the DescribeGeneratedTemplate
//! API action". So everything that read reports lives on ONE [`StoredGeneratedTemplate`]
//! and the [`GeneratedResource`] rows under it, and the read is served from the record
//! rather than from the request.
//!
//! ## The generation rides the clock
//!
//! The generation is ASYNCHRONOUS: the call is answered while the template is still being
//! generated (the modelled `GeneratedTemplateStatus` has a `CREATE_IN_PROGRESS` value whose
//! documentation is "the creation of the template is in progress") and it reaches `COMPLETE`
//! a moment later. A record therefore carries the terminal status the clock owes in
//! [`StoredGeneratedTemplate::pending`], and
//! [`crate::state::ComAmazonawsCloudformationCloudformationState::settle`] is what
//! materialises it: a caller reading straight after the call still sees the generation in
//! flight, one reading after the due instant sees it complete, and nothing sleeps or
//! advances a generation because it was read.
//!
//! ## The in-flight cap
//!
//! How many generations may be in flight AT ONCE is metered, and the quota is modelled:
//! `ConcurrentResourcesLimitExceededException` (HTTP 429, awsQuery code
//! `ConcurrentResourcesLimitExceeded`) is documented "No more than
//! 5 generated templates can be in an InProgress or Pending status at one time". That
//! documented number is [`MAX_CONCURRENT_GENERATIONS`], the set it meters is the records
//! [`StoredGeneratedTemplate::is_in_flight`] answers true for, and because the statuses it
//! counts are the ones the clock retires, the cap LIFTS on its own as the generations it
//! counted complete.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

/// The `GeneratedTemplateStatus` values a generated template reports (the model's own enum,
/// documented value by value on `DescribeGeneratedTemplateOutput$Status`).
///
/// `COMPLETE` ("the template operation is complete") and `FAILED` ("the template operation
/// failed") are the terminal ones; every other value is a generation still in flight, which
/// is what [`StoredGeneratedTemplate::is_in_flight`] and so the in-flight cap key on.
pub mod status {
    pub const CREATE_PENDING: &str = "CREATE_PENDING";
    pub const CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
    pub const UPDATE_PENDING: &str = "UPDATE_PENDING";
    pub const UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
    pub const DELETE_PENDING: &str = "DELETE_PENDING";
    pub const DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
    pub const COMPLETE: &str = "COMPLETE";
    pub const FAILED: &str = "FAILED";
}

/// The `GeneratedTemplateResourceStatus` values one resource of a generated template
/// reports (the model's enum: `PENDING | IN_PROGRESS | FAILED | COMPLETE`), as
/// `ResourceDetail$ResourceStatus`.
pub mod resource_status {
    pub const IN_PROGRESS: &str = "IN_PROGRESS";
    pub const COMPLETE: &str = "COMPLETE";
}

/// The most generations that may be in a pending or in-progress status at one time, in one
/// Region.
///
/// The number the modelled `ConcurrentResourcesLimitExceededException` documents: "No more
/// than 5 generated templates can be in an InProgress or Pending status at one time". A
/// call beyond it is refused with that error and leaves no record behind.
pub const MAX_CONCURRENT_GENERATIONS: usize = 5;

/// The most generated templates one Region holds, past which `CreateGeneratedTemplate`
/// answers the modelled `LimitExceededException` ("The quota for the resource has already
/// been reached").
///
/// A caller chooses how many it creates, so the store bounds them by count. Chosen, like
/// every count in [`crate::state`] that no published quota fixes; it sits far above any
/// client workflow. What frees a record is the caller deleting it, the way a stack's
/// quota is freed by deleting the stack — `DeleteGeneratedTemplate` is still a scaffold
/// stub with its own issue, so nothing reclaims a record yet.
pub const MAX_GENERATED_TEMPLATES: usize = 100;

/// The most generated templates this store holds across EVERY Region together.
///
/// The Region a request is served in comes from its own credential scope, so it is a value
/// the CALLER chooses: a per-Region bound alone would cap only one factor of the store's
/// growth, since a caller signing for one Region string after another could keep creating
/// records. This is the flat ceiling on the product, past which `CreateGeneratedTemplate`
/// answers the same modelled `LimitExceededException` — the same shape
/// [`crate::state::MAX_LIVE_STACKS`] is to [`crate::state::MAX_STACKS`].
pub const MAX_STORED_GENERATED_TEMPLATES: usize = 5 * MAX_GENERATED_TEMPLATES;

/// The most resources one generated template carries.
///
/// Both modelled resource lists of this feature are `@length(min: 1, max: 500)` —
/// `CreateGeneratedTemplateInput$Resources` and `DescribeGeneratedTemplateOutput$Resources`
/// — so 500 is the most a generated template can hold or report.
pub const MAX_GENERATED_TEMPLATE_RESOURCES: usize = 500;

/// How long after the call that starts it a generation's completion becomes due.
///
/// Chosen (nothing documents how long generating a template takes): non-zero, so a caller
/// polling `DescribeGeneratedTemplate` straight after the call still sees the generation in
/// flight — and so the in-flight cap it counts against — and short enough that the poll
/// after it sees the generation complete.
pub const GENERATION_DELAY: Duration = Duration::from_millis(500);

/// The `StatusReason` a generation that completed carries, as the service reports it for a
/// template whose generation is `COMPLETE`.
pub const GENERATION_COMPLETE_REASON: &str = "Template complete";

/// One resource of a generated template: what `ResourceDetail` reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GeneratedResource {
    /// `ResourceDetail$ResourceType`: "The type of the resource, such as
    /// `AWS::DynamoDB::Table`".
    pub resource_type: String,
    /// `ResourceDetail$LogicalResourceId`: "The logical id for this resource in the final
    /// generated template" — the one the base stack declares it under, or the one the
    /// request named for a resource it listed itself. `None` for a listed resource the
    /// request gave none for, since the member is optional and nothing here invents one.
    pub logical_resource_id: Option<String>,
    /// `ResourceDetail$ResourceIdentifier`: the primary identifier that identifies the
    /// resource, keyed by the identifier's PROPERTY name.
    ///
    /// Carries what the request supplied for a resource it listed
    /// (`ResourceDefinition$ResourceIdentifier` is required there, so a listed resource
    /// always has one). EMPTY for a resource taken from the base stack: which property
    /// holds a type's primary identifier is not something this emulator's per-type
    /// catalogue records (the same gap
    /// [`crate::state::resource_schema`](crate::state::resource_schema) leaves
    /// `primaryIdentifier` out of a published schema for), so the member is reported absent
    /// rather than under a property name nothing stands behind.
    pub resource_identifier: BTreeMap<String, String>,
}

/// The terminal status a generation is headed for, and the instant it is due.
#[derive(Debug, Clone)]
pub struct PendingGeneration {
    pub status: String,
    pub status_reason: Option<String>,
    pub due_at: SystemTime,
}

/// One generated template, as this emulator persists it.
#[derive(Debug, Clone)]
pub struct StoredGeneratedTemplate {
    /// `GeneratedTemplateId`: "The Amazon Resource Name (ARN) of the generated template.
    /// The format is
    /// `arn:${Partition}:cloudformation:${Region}:${Account}:generatedtemplate/${Id}`."
    pub generated_template_id: String,
    pub generated_template_name: String,
    /// The Region the generated template lives in: the one the call that created it was
    /// signed for. A name addresses one record of THAT Region, and the record's own ARN
    /// carries the same Region.
    pub region: String,
    /// `StackId`: "The stack ARN of the base stack if a base stack was provided when
    /// generating the template."
    pub stack_id: Option<String>,
    pub resources: Vec<GeneratedResource>,
    /// [`status`].
    pub status: String,
    pub status_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_updated_time: SystemTime,
    /// `TemplateConfiguration$DeletionPolicy`, as the request supplied it.
    pub deletion_policy: Option<String>,
    /// `TemplateConfiguration$UpdateReplacePolicy`, as the request supplied it.
    pub update_replace_policy: Option<String>,
    /// The terminal status the clock owes this generation, until
    /// [`crate::state::ComAmazonawsCloudformationCloudformationState::settle`] materialises
    /// it.
    pub pending: Option<PendingGeneration>,
}

impl StoredGeneratedTemplate {
    /// A generated template as `CreateGeneratedTemplate` first stores it: in flight, with
    /// its completion due one [`GENERATION_DELAY`] from the call that started it.
    pub fn starting(
        generated_template_id: String,
        generated_template_name: String,
        region: String,
        now: SystemTime,
    ) -> Self {
        Self {
            generated_template_id,
            generated_template_name,
            region,
            stack_id: None,
            resources: Vec::new(),
            status: status::CREATE_IN_PROGRESS.to_string(),
            status_reason: None,
            creation_time: now,
            last_updated_time: now,
            deletion_policy: None,
            update_replace_policy: None,
            pending: Some(PendingGeneration {
                status: status::COMPLETE.to_string(),
                status_reason: Some(GENERATION_COMPLETE_REASON.to_string()),
                due_at: now + GENERATION_DELAY,
            }),
        }
    }

    /// Materialise the status transition the clock has already made due, if any.
    ///
    /// Only ever applies a transition whose due instant has passed, so it reports what was
    /// owed at `now` even if nobody had called.
    pub fn settle(&mut self, now: SystemTime) {
        if self.pending.as_ref().is_none_or(|p| p.due_at > now) {
            return;
        }
        let pending = self.pending.take().expect("checked just above");
        self.status = pending.status;
        self.status_reason = pending.status_reason;
        // "The time the generated template was last updated": the generation concluding is
        // the update, so it is stamped with the instant that was due rather than with now.
        self.last_updated_time = pending.due_at;
    }

    /// Whether this generation is still in flight — a pending or in-progress status, which
    /// is the set [`MAX_CONCURRENT_GENERATIONS`] meters.
    pub fn is_in_flight(&self) -> bool {
        !matches!(self.status.as_str(), status::COMPLETE | status::FAILED)
    }

    /// Whether `name_or_id` addresses this record in `region`: `GeneratedTemplateName` is
    /// "the name or Amazon Resource Name (ARN) of a generated template".
    ///
    /// Nothing outside the request's own Region is addressable — the same name in another
    /// Region is another generated template, exactly as it is for a stack
    /// ([`crate::state::ComAmazonawsCloudformationCloudformationState::resolve`]).
    pub fn is_addressed_by(&self, region: &str, name_or_id: &str) -> bool {
        self.region == region
            && (self.generated_template_name == name_or_id
                || self.generated_template_id == name_or_id)
    }

    /// The `ResourceStatus` every resource of this template reports: the resources of a
    /// generation that has concluded are complete, and those of one still in flight are
    /// being processed.
    pub fn resource_status(&self) -> &'static str {
        if self.is_in_flight() {
            resource_status::IN_PROGRESS
        } else {
            resource_status::COMPLETE
        }
    }

    /// `TemplateProgress`: "a summary of the progress of the template generation", counted
    /// over the resources this template carries.
    pub fn progress(&self) -> GenerationProgress {
        let resources = i32::try_from(self.resources.len()).unwrap_or(i32::MAX);
        let in_flight = self.is_in_flight();
        GenerationProgress {
            resources_succeeded: if in_flight { 0 } else { resources },
            resources_processing: if in_flight { resources } else { 0 },
            // A generation here either concludes for every resource it covers or does not
            // conclude yet: no resource of it is left queued behind another, and none of
            // them fails on its own.
            resources_pending: 0,
            resources_failed: 0,
        }
    }
}

/// The counts `TemplateProgress` reports for one generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationProgress {
    pub resources_succeeded: i32,
    pub resources_failed: i32,
    pub resources_processing: i32,
    pub resources_pending: i32,
}

/// The message answered when a request names a generated template this account and Region
/// holds no record of, built from what the request named.
pub fn does_not_exist(name_or_id: &str) -> String {
    format!("Generated template {name_or_id} cannot be found")
}
