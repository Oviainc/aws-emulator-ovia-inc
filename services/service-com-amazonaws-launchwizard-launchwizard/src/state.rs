// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-launchwizard-launchwizard service.
//!
//! This module owns everything the operation handlers share: the seeded workload
//! catalogue, the authoritative deployment store, the clock-owed status steps a
//! deployment still has to take, and the small projection/pagination helpers the
//! handlers in `ops/` call. `lib.rs` is generated, so a service cannot add a module
//! beside this one — shared service logic lives here rather than being copied into
//! each operation file.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use ::aws_smithy_schema_server::constraints::{Violation, ViolationKind};

use crate::ops::types::*;

/// The region an ARN carries when the request is unsigned and therefore names no
/// SigV4 credential scope to read it from (the emulator's default region, as
/// `emulator_server::emulator_arn` documents).
pub const DEFAULT_REGION: &str = "us-east-1";

/// This service's ARN namespace, from `OPERATIONS.md` ("ARN namespace: launchwizard").
pub const ARN_SERVICE: &str = "launchwizard";

/// How long one clock-owed deployment status step takes.
///
/// Launch Wizard documents no duration for a deployment reaching `COMPLETED`, so
/// this span is CHOSEN: it is non-zero, so a caller that reads straight after
/// `CreateDeployment` still observes `CREATING`, and short enough that a caller
/// that waits observes the terminal status.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// `DeploymentStatus` members this slice moves a deployment through. Every value is
/// one of `DEPLOYMENT_STATUS_VALUES` (the model's `DeploymentStatus` enum).
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_CREATING`].
pub const STATUS_COMPLETED: &str = "COMPLETED";
/// See [`STATUS_CREATING`].
///
/// `UPDATE_IN_PROGRESS` and `UPDATE_COMPLETED` are the two `DeploymentStatus` members
/// the model declares for an UPDATE of a deployment, which is why an update moves a
/// deployment through them rather than through the create pair.
pub const STATUS_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";
/// See [`STATUS_UPDATE_IN_PROGRESS`].
pub const STATUS_UPDATE_COMPLETED: &str = "UPDATE_COMPLETED";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETE_IN_PROGRESS: &str = "DELETE_IN_PROGRESS";
/// See [`STATUS_CREATING`].
///
/// The status a delete that has only just been accepted is in, and so the one
/// `DeleteDeployment` reports synchronously; the deployment then advances on the
/// clock to [`STATUS_DELETE_IN_PROGRESS`] and finally [`STATUS_DELETED`].
pub const STATUS_DELETE_INITIATING: &str = "DELETE_INITIATING";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETED: &str = "DELETED";

/// `EventStatus` members this slice records on a deployment event. Every value is one
/// of `EVENT_STATUS_VALUES` (the model's `EventStatus` enum).
pub const EVENT_CREATED: &str = "CREATED";
/// See [`EVENT_CREATED`].
pub const EVENT_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`EVENT_CREATED`].
pub const EVENT_COMPLETED: &str = "COMPLETED";
/// See [`EVENT_CREATED`].
///
/// The step a request has been accepted for but not started yet — what a delete that
/// has only reached [`STATUS_DELETE_INITIATING`] has done so far.
pub const EVENT_PENDING: &str = "PENDING";

/// `WorkloadStatus` member the seeded catalogue entries carry, from
/// `WORKLOAD_STATUS_VALUES`.
pub const WORKLOAD_ACTIVE: &str = "ACTIVE";

/// `WorkloadDeploymentPatternStatus` member the seeded deployment patterns carry,
/// from `WORKLOAD_DEPLOYMENT_PATTERN_STATUS_VALUES`.
pub const PATTERN_ACTIVE: &str = "ACTIVE";

/// The two values `DeploymentSpecificationsField$required` carries.
///
/// The model types the member as a plain `String` and documents it as "Indicates if
/// the deployment specification is required", so the vocabulary is not an enum: these
/// are the `Yes` / `No` values Launch Wizard's deployment specifications use, the same
/// pair the model's own `CreateDeployment` example submits for its boolean
/// specifications (`DisableDeploymentRollback: "Yes"`, `SaveDeploymentArtifacts:
/// "No"`).
pub const REQUIRED_YES: &str = "Yes";
/// See [`REQUIRED_YES`].
pub const REQUIRED_NO: &str = "No";

/// A `DeploymentConditionalField$comparator` value. The model documents the member as
/// "Valid values: `Equal | NotEqual`".
pub const COMPARATOR_EQUAL: &str = "Equal";
/// See [`COMPARATOR_EQUAL`].
pub const COMPARATOR_NOT_EQUAL: &str = "NotEqual";

/// The `workloadVersionName` every seeded catalogue entry reports.
///
/// `WorkloadVersionName` / `DeploymentPatternVersionName` are len 5..30 with pattern
/// `^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$`; this is the dotted alternative at
/// its shortest. The catalogue is service-owned (no API creates a workload or a
/// pattern), so the emulator seeds ONE version per workload; a pattern's own versions
/// are [`CATALOGUE_PATTERN_VERSIONS`].
pub const CATALOGUE_VERSION: &str = "1.0.0";

/// The `DeploymentPatternVersionName`s the seeded catalogue publishes for EVERY
/// deployment pattern, OLDEST FIRST — the last entry is the version the pattern
/// currently deploys, which is the one `ListWorkloadDeploymentPatterns` and
/// `GetWorkloadDeploymentPattern` report as its `deploymentPatternVersionName`.
///
/// The catalogue is service-owned (there is no API that creates a pattern or a version
/// of one), so this list is the EMULATOR's own rather than a claim about which versions
/// AWS publishes; each entry stays inside `DeploymentPatternVersionName` (see
/// [`CATALOGUE_VERSION`]) using the dotted alternative. More than one version is seeded
/// because a pattern's version operations (`GetDeploymentPatternVersion`,
/// `ListDeploymentPatternVersions`) exist to tell the versions of ONE pattern apart,
/// which a catalogue holding a single version per pattern cannot show.
pub const CATALOGUE_PATTERN_VERSIONS: &[&str] = &["1.0.0", "1.1.0", "2.0.0"];

/// The message an update of a deployment that is going away is refused with.
///
/// A deployment whose delete has started no longer has settings to apply, so an
/// update of it is refused rather than applied. Fixed and identifier-free like the
/// not-found messages above; AWS documents no wording for it, so this text is CHOSEN
/// and only the status (HTTP 400) and the modelled `ValidationException` code are the
/// contract.
pub const DEPLOYMENT_NOT_UPDATABLE_MESSAGE: &str =
    "The specified deployment is being deleted and cannot be updated.";

/// The message a deployment lookup that finds nothing carries.
///
/// Launch Wizard's `ResourceNotFoundException` message is FIXED per kind of resource
/// looked up and never renders the identifier the caller asked for (the existence of
/// an id is not disclosed through the error channel), so every handler that resolves
/// a `deploymentId` answers with this constant rather than a per-request template.
/// This text is Launch Wizard's recorded wording for a deployment lookup.
pub const DEPLOYMENT_NOT_FOUND_MESSAGE: &str = "The specified deployment does not exist.";

/// The message a workload-catalogue lookup that finds nothing carries.
///
/// See [`DEPLOYMENT_NOT_FOUND_MESSAGE`]: identifier-free and fixed. Launch Wizard's
/// recorded wording for a workload lookup deliberately conflates "absent" with "you
/// may not see it", so a caller cannot probe the catalogue through the error channel.
pub const WORKLOAD_NOT_FOUND_MESSAGE: &str =
    "The specified workload does not exist or insufficient permissions.";

/// The message a deployment-pattern lookup that finds nothing carries.
///
/// Neither AWS's docs nor the recordings state the wording for a pattern lookup, so
/// this text is CHOSEN; what the contract requires, and what this constant exists to
/// hold, is that the message is fixed and identifier-free like the two above, which a
/// pattern name (caller input, and a probe of the catalogue) must not breach either.
pub const DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE: &str =
    "The specified deployment pattern does not exist or insufficient permissions.";

/// The message a deployment-pattern-VERSION lookup that finds nothing carries.
///
/// A version of a deployment pattern is its own resource (its own name, description and
/// read operations), so a pattern that exists at a version the catalogue does not
/// publish is refused with its own message rather than with the pattern's. Fixed and
/// identifier-free for the reason [`DEPLOYMENT_PATTERN_NOT_FOUND_MESSAGE`] gives, and
/// CHOSEN for the same reason: neither AWS's docs nor the recordings state the wording.
pub const DEPLOYMENT_PATTERN_VERSION_NOT_FOUND_MESSAGE: &str =
    "The specified deployment pattern version does not exist or insufficient permissions.";

/// How many deployment records this emulator keeps at once.
///
/// AWS publishes no deployment quota, so this is the EMULATOR's own bound, not a
/// claim about the service: the store lives for the life of the process and a caller
/// can create records in it, so it needs a ceiling. Reaching it first drops the
/// oldest already-deleted records (terminal, and retained only as a courtesy); a
/// create that still cannot fit is answered with the modelled
/// `ResourceLimitException`, which is exactly what Launch Wizard answers when a
/// deployment limit is exceeded.
pub const MAX_DEPLOYMENTS: usize = 200;

/// How many events ONE deployment's history keeps.
///
/// AWS publishes no bound on a deployment's event history, so this is the EMULATOR's
/// own ceiling, not a claim about the service: the store lives for the life of the
/// process and `UpdateDeployment` can be called on the same deployment any number of
/// times, each call appending to that deployment's log, so the log needs a ceiling of
/// its own — the number of deployment RECORDS ([`MAX_DEPLOYMENTS`]) does not bound it.
/// Reaching it drops the OLDEST events, the way the other per-resource event logs in
/// this workspace are bounded (cloudformation's `MAX_EVENTS_PER_STACK`), so the most
/// recent history — what a caller polls for — is the part that is kept.
pub const MAX_EVENTS_PER_DEPLOYMENT: usize = 200;

/// The page size a list operation uses when the caller names no `maxResults`.
///
/// The model caps `maxResults` at 100 (`@range(min: 1, max: 100)`), so one page
/// never carries more than that whether the caller asked for a size or not, and the
/// rest is reached through `nextToken`.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// How many tags one resource can carry.
///
/// The model's `Tags` map is `@length(min: 1, max: 200)`, and a resource's tag set IS
/// a `Tags` map — it is what `ListTagsForResource` reports — so the set a `TagResource`
/// leaves behind keeps that ceiling, not just the one map a single request carries.
/// AWS publishes no separate per-resource tag quota for Launch Wizard, so this is the
/// MODELLED ceiling rather than a documented quota.
pub const MAX_TAGS_PER_RESOURCE: usize = 200;

/// The `ValidationException` message every unusable `nextToken` is refused with.
///
/// A pagination token is OPAQUE — the caller may only hand back a token a previous
/// page gave it — so the refusal is one fixed sentence (recorded behaviour) that
/// states neither the rejected token nor which way it was unusable: a token this
/// service never issued and a token that no longer names a position in the result set
/// are the same refusal to a caller, and echoing the token back would put
/// caller-supplied input in the error channel.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "NextToken is not valid.";

/// One conditional on a deployment specification field: the OTHER field whose value
/// decides whether this one is read.
#[derive(Debug, Clone)]
pub struct SpecificationConditional {
    /// `DeploymentConditionalField$name` — the name of the field this condition reads.
    pub name: String,
    /// `DeploymentConditionalField$value`.
    pub value: String,
    /// `DeploymentConditionalField$comparator`, one of [`COMPARATOR_EQUAL`] /
    /// [`COMPARATOR_NOT_EQUAL`].
    pub comparator: String,
}

impl SpecificationConditional {
    /// This conditional as the modelled `DeploymentConditionalField`.
    pub fn to_field(&self) -> DeploymentConditionalField {
        DeploymentConditionalField {
            name: Some(self.name.clone()),
            value: Some(self.value.clone()),
            comparator: Some(self.comparator.clone()),
        }
    }
}

/// One field of a deployment pattern's specification schema: what a caller may put in
/// `CreateDeployment`'s `specifications` map for this pattern.
///
/// `name` is a `DeploymentSpecifications` KEY (len 3..256, `^[a-zA-Z0-9-:]+$`), so a
/// caller can submit the reported name as it stands.
#[derive(Debug, Clone)]
pub struct SpecificationField {
    pub name: String,
    pub description: String,
    /// `DeploymentSpecificationsField$allowedValues`: empty when the field takes any
    /// `ValueString`, in which case nothing is reported rather than an empty list.
    pub allowed_values: Vec<String>,
    /// Reported as [`REQUIRED_YES`] / [`REQUIRED_NO`].
    pub required: bool,
    /// `DeploymentSpecificationsField$conditionals`: empty when the field is read
    /// unconditionally.
    pub conditionals: Vec<SpecificationConditional>,
}

impl SpecificationField {
    /// This field as the modelled `DeploymentSpecificationsField`.
    pub fn to_field(&self) -> DeploymentSpecificationsField {
        DeploymentSpecificationsField {
            name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            allowed_values: (!self.allowed_values.is_empty())
                .then(|| self.allowed_values.clone()),
            required: Some(
                if self.required {
                    REQUIRED_YES
                } else {
                    REQUIRED_NO
                }
                .to_string(),
            ),
            conditionals: (!self.conditionals.is_empty()).then(|| {
                self.conditionals
                    .iter()
                    .map(SpecificationConditional::to_field)
                    .collect()
            }),
        }
    }
}

/// One VERSION of a deployment pattern, in the seeded, read-only catalogue.
///
/// This is the single record every deployment-pattern-version view reports from
/// (`GetDeploymentPatternVersion` today), so a version's name and description cannot
/// differ between two readers. The workload and pattern a version belongs to are NOT
/// stored here: they are read off the [`DeploymentPattern`] that holds it (see
/// [`DeploymentPattern::version_summary`]), so a version can never name a pattern it is
/// not part of.
#[derive(Debug, Clone)]
pub struct PatternVersion {
    /// `DeploymentPatternVersionName`: len 5..30,
    /// `^(([A-Za-z0-9][a-zA-Z0-9-]*)|(\d+\.\d+\.\d+))$`. One of
    /// [`CATALOGUE_PATTERN_VERSIONS`].
    pub version_name: String,
    /// `DeploymentPatternVersionDataSummary$description`.
    pub description: String,
}

/// A deployment pattern of a workload, in the seeded, read-only catalogue.
///
/// This is the single record `ListWorkloadDeploymentPatterns` (summaries) and
/// `GetWorkloadDeploymentPattern` (the same members plus the specification schema)
/// both report from, so the two can never disagree.
#[derive(Debug, Clone)]
pub struct DeploymentPattern {
    /// The `WorkloadName` this pattern belongs to.
    pub workload_name: String,
    /// `DeploymentPatternName`: len 1..256, `^[A-Za-z0-9][a-zA-Z0-9-]*$`.
    pub pattern_name: String,
    /// `WorkloadDeploymentPatternData$displayName`.
    pub display_name: String,
    /// `WorkloadDeploymentPatternData$description`.
    pub description: String,
    /// One of `WORKLOAD_DEPLOYMENT_PATTERN_STATUS_VALUES`.
    pub status: String,
    /// `WorkloadDeploymentPatternData$statusMessage`: present only when the status
    /// needs explaining, which [`PATTERN_ACTIVE`] does not.
    pub status_message: Option<String>,
    /// `workloadVersionName`, see [`CATALOGUE_VERSION`].
    pub workload_version_name: String,
    /// Every version of this pattern the catalogue publishes, OLDEST FIRST — see
    /// [`CATALOGUE_PATTERN_VERSIONS`]. The pattern's current version is derived from
    /// this list ([`DeploymentPattern::current_version`]) rather than stored beside it,
    /// so the version a pattern reports is always one the catalogue holds.
    pub versions: Vec<PatternVersion>,
    /// The specification schema a `CreateDeployment` for this pattern fills in.
    pub specifications: Vec<SpecificationField>,
}

impl DeploymentPattern {
    /// The version this pattern currently deploys: the NEWEST version the catalogue
    /// publishes for it, which is the one both pattern views report as
    /// `deploymentPatternVersionName`. `None` only for a pattern with no versions at
    /// all, which the seeded catalogue never builds.
    pub fn current_version(&self) -> Option<&PatternVersion> {
        self.versions.last()
    }

    /// The version of this pattern named `version_name`, or `None` when the catalogue
    /// publishes no such version of it.
    ///
    /// The lookup is over the published list, so "a version this pattern has" means the
    /// same thing to every caller — the reads that report a version and the update that
    /// moves a deployment onto one.
    pub fn version(&self, version_name: &str) -> Option<&PatternVersion> {
        self.versions
            .iter()
            .find(|version| version.version_name == version_name)
    }

    /// One of this pattern's versions as the `DeploymentPatternVersionDataSummary`
    /// `GetDeploymentPatternVersion` returns, or `None` when the catalogue publishes no
    /// version of this pattern under that name.
    ///
    /// `workloadName` and `deploymentPatternName` are read off THIS record rather than
    /// off the request, so the reported version is always stated as a version of the
    /// pattern it was found in. `documentationUrl` is absent for the reason
    /// [`Workload::to_data`] gives: this emulator hosts no document for a catalogue
    /// entry, so there is no URL to report.
    pub fn version_summary(
        &self,
        version_name: &str,
    ) -> Option<DeploymentPatternVersionDataSummary> {
        let version = self.version(version_name)?;
        Some(DeploymentPatternVersionDataSummary {
            deployment_pattern_version_name: Some(version.version_name.clone()),
            description: Some(version.description.clone()),
            documentation_url: None,
            workload_name: Some(self.workload_name.clone()),
            deployment_pattern_name: Some(self.pattern_name.clone()),
        })
    }

    /// This pattern as the `WorkloadDeploymentPatternDataSummary`
    /// `ListWorkloadDeploymentPatterns` returns.
    ///
    /// `accountConstraints` is left absent for the same reason as on a workload: this
    /// emulator models no account constraints, so reporting none is the honest answer.
    pub fn to_summary(&self) -> WorkloadDeploymentPatternDataSummary {
        WorkloadDeploymentPatternDataSummary {
            workload_name: Some(self.workload_name.clone()),
            deployment_pattern_name: Some(self.pattern_name.clone()),
            workload_version_name: Some(self.workload_version_name.clone()),
            deployment_pattern_version_name: self
                .current_version()
                .map(|version| version.version_name.clone()),
            display_name: Some(self.display_name.clone()),
            description: Some(self.description.clone()),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            account_constraints: None,
        }
    }

    /// This pattern as the `WorkloadDeploymentPatternData`
    /// `GetWorkloadDeploymentPattern` returns: every member of the summary, plus the
    /// specification schema only the detail view carries.
    pub fn to_data(&self) -> WorkloadDeploymentPatternData {
        WorkloadDeploymentPatternData {
            workload_name: Some(self.workload_name.clone()),
            deployment_pattern_name: Some(self.pattern_name.clone()),
            workload_version_name: Some(self.workload_version_name.clone()),
            deployment_pattern_version_name: self
                .current_version()
                .map(|version| version.version_name.clone()),
            display_name: Some(self.display_name.clone()),
            description: Some(self.description.clone()),
            status: Some(self.status.clone()),
            status_message: self.status_message.clone(),
            account_constraints: None,
            specifications: Some(
                self.specifications
                    .iter()
                    .map(SpecificationField::to_field)
                    .collect(),
            ),
        }
    }
}

/// A workload in the seeded, read-only catalogue `CreateDeployment` resolves against.
///
/// `patterns` holds the full per-pattern record (display name, description, status,
/// versions and specification schema), keyed by `DeploymentPatternName`, so the
/// operations that report a pattern and the one that deploys it read the SAME entry.
#[derive(Debug, Clone)]
pub struct Workload {
    /// `WorkloadName`: len 1..100, `^[A-Za-z][a-zA-Z0-9-_]*$`.
    pub workload_name: String,
    /// `WorkloadDataSummary$displayName`.
    pub display_name: String,
    /// `WorkloadData$description`: what the workload deploys.
    pub description: String,
    /// `WorkloadDataSummary$status`, one of `WORKLOAD_STATUS_VALUES`.
    pub status: String,
    /// The `DeploymentPatternName`s this workload supports (len 1..256,
    /// `^[A-Za-z0-9][a-zA-Z0-9-]*$`), each with its catalogue record.
    pub patterns: BTreeMap<String, DeploymentPattern>,
}

impl Workload {
    /// This workload as the `WorkloadDataSummary` `ListWorkloads` returns.
    ///
    /// `accountConstraints` is left absent: the model documents it as the constraints
    /// on which kind of account may deploy the workload, and this emulator models no
    /// account constraints, so reporting none is the honest answer.
    pub fn to_summary(&self) -> WorkloadDataSummary {
        WorkloadDataSummary {
            workload_name: Some(self.workload_name.clone()),
            display_name: Some(self.display_name.clone()),
            status: Some(self.status.clone()),
            account_constraints: None,
        }
    }

    /// This workload as the `WorkloadData` `GetWorkload` returns.
    ///
    /// The members `WorkloadData` shares with `WorkloadDataSummary` are read off the
    /// same catalogue entry [`Workload::to_summary`] reads, so the two reporting
    /// operations cannot disagree about one workload.
    ///
    /// `accountConstraints` is absent for the reason [`Workload::to_summary`] gives.
    /// `documentationUrl` and `iconUrl` are absent because this emulator hosts no
    /// workload document and no icon, so there is no URL to report; `statusMessage` is
    /// absent because an `ACTIVE` catalogue entry has nothing to say about its status.
    pub fn to_data(&self) -> WorkloadData {
        WorkloadData {
            workload_name: Some(self.workload_name.clone()),
            display_name: Some(self.display_name.clone()),
            status: Some(self.status.clone()),
            account_constraints: None,
            description: Some(self.description.clone()),
            documentation_url: None,
            icon_url: None,
            status_message: None,
        }
    }
}

/// One event in a deployment's history, appended by the lifecycle calls (and by the
/// clock-owed steps those calls schedule) — never canned.
#[derive(Debug, Clone)]
pub struct DeploymentEvent {
    /// `DeploymentEventDataSummary$name`.
    pub name: String,
    /// `DeploymentEventDataSummary$description`.
    pub description: String,
    /// `DeploymentEventDataSummary$status`, one of `EVENT_STATUS_VALUES`.
    pub status: String,
    /// `DeploymentEventDataSummary$statusReason`: present only when the event carries
    /// one, which the happy path does not.
    pub status_reason: Option<String>,
    /// When the event happened.
    pub timestamp: SystemTime,
}

impl DeploymentEvent {
    /// A successful lifecycle event at `timestamp`.
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        status: &str,
        timestamp: SystemTime,
    ) -> Self {
        Self {
            name: name.into(),
            description: description.into(),
            status: status.to_string(),
            status_reason: None,
            timestamp,
        }
    }

    /// This event as the `DeploymentEventDataSummary` `ListDeploymentEvents` returns.
    ///
    /// `metadata` is left absent: the emulator attaches no provider metadata to an
    /// event, so there is nothing to report.
    pub fn to_summary(&self) -> DeploymentEventDataSummary {
        DeploymentEventDataSummary {
            name: Some(self.name.clone()),
            description: Some(self.description.clone()),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            timestamp: Some(to_date_time(self.timestamp)),
            metadata: None,
        }
    }
}

/// A deployment status step the CLOCK owes, scheduled by the call that started it.
///
/// It is materialised by the first request at or after `due_at` — never by a sleep,
/// and never early: a caller acting immediately after `CreateDeployment` still sees
/// `CREATING` and a caller acting immediately after `DeleteDeployment` still sees
/// `DELETE_INITIATING`.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// The instant this step became owed.
    pub due_at: SystemTime,
    /// The `DeploymentStatus` the deployment takes when the step is materialised.
    pub status: String,
    /// The event appended when the step is materialised.
    pub event: DeploymentEvent,
}

/// A deployment: the service's primary resource and the authoritative record every
/// response field about it is computed from.
#[derive(Debug, Clone)]
pub struct Deployment {
    /// `DeploymentId`: len 2..128, `^[a-zA-Z0-9-]+$`.
    pub id: String,
    /// The order this record was created in, from
    /// [`ComAmazonawsLaunchwizardLaunchwizardState::next_creation_sequence`].
    ///
    /// `created_at` alone cannot order two deployments created within one clock tick,
    /// and a minted id is a random UUID rather than a monotonic one, so the order
    /// `ListDeployments` pages over is this counter — strictly increasing and never
    /// reported on the wire.
    pub sequence: u64,
    /// `DeploymentName` as submitted: len 1..50, `^[A-Za-z0-9_\.-]+$`.
    pub name: String,
    pub workload_name: String,
    pub pattern_name: String,
    /// One of `DEPLOYMENT_STATUS_VALUES`.
    pub status: String,
    /// `DeleteDeploymentOutput$statusReason` / `DeploymentData` has no reason in the
    /// happy path: the emulator reaches no failure status, so there is nothing to say.
    pub status_reason: Option<String>,
    /// The submitted `specifications` (the model marks the map `@sensitive`).
    pub specifications: BTreeMap<String, String>,
    /// `DeploymentData$resourceGroup`. Launch Wizard reports the Resource Groups group
    /// it created for the deployment; this emulator creates none, so it stays absent
    /// rather than naming a group that does not exist.
    pub resource_group: Option<String>,
    pub deployment_arn: String,
    /// Tags live here, on the deployment, keyed by nothing else — so the tagging
    /// operations can be layered on without a second store.
    pub tags: BTreeMap<String, String>,
    /// The `WorkloadVersionName` this deployment deploys: the catalogue version of its
    /// workload at create time, and whatever version `UpdateDeployment` last named.
    ///
    /// No output shape in this model carries a deployment's version (neither
    /// `DeploymentData` nor `DeploymentDataSummary` declares one), so it is observable
    /// through the deployment event an update appends rather than as a response member.
    pub workload_version_name: String,
    /// The `DeploymentPatternVersionName` this deployment deploys. See
    /// [`Deployment::workload_version_name`].
    pub pattern_version_name: String,
    pub created_at: SystemTime,
    pub modified_at: SystemTime,
    pub deleted_at: Option<SystemTime>,
    pub events: Vec<DeploymentEvent>,
    /// Status steps the clock still owes, oldest due first.
    pub pending: VecDeque<PendingTransition>,
}

impl Deployment {
    /// Append `event` to this deployment's history, dropping the OLDEST events once
    /// the log is over [`MAX_EVENTS_PER_DEPLOYMENT`].
    ///
    /// EVERY append goes through here: the log is the one part of a deployment record
    /// that a repeatable call can grow, so the ceiling is enforced where the growth
    /// happens rather than at each caller.
    pub fn append_event(&mut self, event: DeploymentEvent) {
        self.events.push(event);
        if self.events.len() > MAX_EVENTS_PER_DEPLOYMENT {
            let overflow = self.events.len() - MAX_EVENTS_PER_DEPLOYMENT;
            self.events.drain(..overflow);
        }
    }

    /// Materialise every status step whose `due_at` has already passed.
    ///
    /// This is the one write a reporting operation is allowed to make: each step was
    /// owed at `due_at` whether or not anybody called, so applying it on read reports
    /// the state the clock already produced. The event timestamp and `modifiedAt` are
    /// the step's own due instant, not "now", so the record does not depend on when it
    /// was observed.
    pub fn materialize_due(&mut self, now: SystemTime) {
        while self.pending.front().is_some_and(|step| step.due_at <= now) {
            let Some(step) = self.pending.pop_front() else {
                break;
            };
            self.status = step.status;
            self.modified_at = step.due_at;
            if self.status == STATUS_DELETED {
                self.deleted_at = Some(step.due_at);
            }
            self.append_event(step.event);
        }
    }

    /// True once the deployment has reached `DELETED`.
    pub fn is_deleted(&self) -> bool {
        self.status == STATUS_DELETED
    }

    /// True while a delete is under way or finished — the states in which another
    /// `DeleteDeployment` must not restart anything.
    pub fn is_deleting_or_deleted(&self) -> bool {
        matches!(
            self.status.as_str(),
            STATUS_DELETE_INITIATING | STATUS_DELETE_IN_PROGRESS | STATUS_DELETED
        )
    }

    /// This deployment as the `DeploymentData` `GetDeployment` returns.
    pub fn to_data(&self) -> DeploymentData {
        DeploymentData {
            name: Some(self.name.clone()),
            id: Some(self.id.clone()),
            workload_name: Some(self.workload_name.clone()),
            pattern_name: Some(self.pattern_name.clone()),
            status: Some(self.status.clone()),
            created_at: Some(to_date_time(self.created_at)),
            modified_at: Some(to_date_time(self.modified_at)),
            specifications: Some(to_hash_map(&self.specifications)),
            resource_group: self.resource_group.clone(),
            deleted_at: self.deleted_at.map(to_date_time),
            tags: (!self.tags.is_empty()).then(|| to_hash_map(&self.tags)),
            deployment_arn: Some(self.deployment_arn.clone()),
        }
    }

    /// This deployment as the `DeploymentDataSummary` `ListDeployments` returns.
    pub fn to_summary(&self) -> DeploymentDataSummary {
        DeploymentDataSummary {
            name: Some(self.name.clone()),
            id: Some(self.id.clone()),
            workload_name: Some(self.workload_name.clone()),
            pattern_name: Some(self.pattern_name.clone()),
            status: Some(self.status.clone()),
            created_at: Some(to_date_time(self.created_at)),
            modified_at: Some(to_date_time(self.modified_at)),
        }
    }
}

/// Per-service state for com-amazonaws-launchwizard-launchwizard. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsLaunchwizardLaunchwizardState {
    /// The seeded, read-only workload catalogue, keyed by `workloadName`.
    pub workloads: BTreeMap<String, Workload>,
    /// The authoritative deployment store, keyed by `deploymentId`.
    pub deployments: BTreeMap<String, Deployment>,
    /// Sequence behind
    /// [`ComAmazonawsLaunchwizardLaunchwizardState::next_creation_sequence`].
    pub next_sequence: u64,
}

impl Default for ComAmazonawsLaunchwizardLaunchwizardState {
    fn default() -> Self {
        Self {
            workloads: seeded_workloads(),
            deployments: BTreeMap::new(),
            next_sequence: 1,
        }
    }
}

impl ComAmazonawsLaunchwizardLaunchwizardState {
    /// Mint a `deploymentId` no stored deployment already carries, or `None` when every
    /// draw of a BOUNDED retry collided with a stored id.
    ///
    /// A Launch Wizard deployment id is a UUID — the form every ARN that names a
    /// deployment (`.../deployment/<deploymentId>`) carries, and the form the tagging
    /// APIs parse a resource ARN against (see [`deployment_id_in_arn`]) — and it stays
    /// inside the `DeploymentId` shape (len 2..128, `^[a-zA-Z0-9-]+$`). A fresh v4 is
    /// random rather than guessable, and the retry keeps the store's keys unique even if
    /// two draws ever collided.
    ///
    /// The retry is bounded by a ceiling of this service's own: the store holds at most
    /// [`MAX_DEPLOYMENTS`] ids, so at most that many draws can collide with one and one
    /// draw more than that cannot. Nothing a caller sends changes the count, and the
    /// `None` an exhausted draw reports is answered with a server error rather than by
    /// drawing again.
    pub fn mint_deployment_id(&self) -> Option<String> {
        (0..=MAX_DEPLOYMENTS)
            .map(|_| fresh_uuid_v4())
            .find(|id| !self.deployments.contains_key(id))
    }

    /// The next `Deployment::sequence`: the position a new record takes in creation
    /// order, which is what [`Self::deployments_in_order`] pages over.
    pub fn next_creation_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence += 1;
        sequence
    }

    /// Materialise the clock-owed steps of every stored deployment. Call this first in
    /// every handler, so each one sees the same, current state.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for deployment in self.deployments.values_mut() {
            deployment.materialize_due(now);
        }
    }

    /// True when a deployment that has not been deleted already carries `name`.
    ///
    /// Derived from the store rather than from a second index, so the two cannot
    /// drift: a deleted deployment keeps its record (and its name on it) but no longer
    /// holds the name against a new deployment.
    pub fn live_name_taken(&self, name: &str) -> bool {
        self.deployments
            .values()
            .any(|d| d.name == name && !d.is_deleted())
    }

    /// Every stored deployment in creation order, oldest first — the order
    /// `ListDeployments` pages over.
    ///
    /// Ordered by `sequence` rather than by `created_at`, so two deployments created
    /// within one clock tick still page in the order they were created in.
    pub fn deployments_in_order(&self) -> Vec<&Deployment> {
        let mut all: Vec<&Deployment> = self.deployments.values().collect();
        all.sort_by_key(|deployment| deployment.sequence);
        all
    }

    /// Make room for one more deployment, reporting whether there is any.
    ///
    /// See [`MAX_DEPLOYMENTS`]: already-deleted records are dropped oldest-first to
    /// fit a new one, and `false` means even that was not enough, which the caller
    /// answers with the modelled `ResourceLimitException`.
    pub fn make_room_for_a_deployment(&mut self) -> bool {
        while self.deployments.len() >= MAX_DEPLOYMENTS {
            let oldest_deleted = self
                .deployments_in_order()
                .into_iter()
                .find(|deployment| deployment.is_deleted())
                .map(|deployment| deployment.id.clone());
            match oldest_deleted {
                Some(id) => {
                    self.deployments.remove(&id);
                }
                None => return false,
            }
        }
        true
    }
}

/// A specification field of the seeded schema: one that takes any `ValueString`, or
/// one of `allowed_values` when that list is not empty.
///
/// A field is built UNREQUIRED here. Whether a field has to be filled in is a property
/// of the deployment PATTERN publishing it, not of the field: one pattern can read a
/// setting another pattern of the same workload insists on. Each pattern therefore
/// states its own required set once ([`SAP_HANA_SINGLE_REQUIRED`] and its siblings),
/// applied over the
/// composed schema by [`mark_required`].
fn spec_field(name: &str, description: &str, allowed_values: &[&str]) -> SpecificationField {
    SpecificationField {
        name: name.to_string(),
        description: description.to_string(),
        allowed_values: allowed_values.iter().map(|v| (*v).to_string()).collect(),
        required: false,
        conditionals: Vec::new(),
    }
}

/// [`spec_field`] for a field that is read only when another field of the same schema
/// carries a particular value.
fn conditional_field(
    name: &str,
    description: &str,
    allowed_values: &[&str],
    conditionals: Vec<SpecificationConditional>,
) -> SpecificationField {
    SpecificationField {
        conditionals,
        ..spec_field(name, description, allowed_values)
    }
}

/// One conditional: this field is read only when the field named `on` compares
/// `comparator` to `value`.
fn conditional(on: &str, comparator: &str, value: &str) -> SpecificationConditional {
    SpecificationConditional {
        name: on.to_string(),
        value: value.to_string(),
        comparator: comparator.to_string(),
    }
}

/// Field names other fields' conditionals read, so a conditional cannot name a field
/// the schema does not publish.
const FIELD_CREATE_SECURITY_GROUP: &str = "CreateSecurityGroup";
/// See [`FIELD_CREATE_SECURITY_GROUP`].
const FIELD_ENABLE_EBS_VOLUME_ENCRYPTION: &str = "EnableEbsVolumeEncryption";
/// See [`FIELD_CREATE_SECURITY_GROUP`].
const FIELD_CLUSTER_QUORUM_TYPE: &str = "ClusterQuorumType";

/// The field names the required set of every seeded pattern holds: a deployment always
/// launches EC2 instances into a VPC, whichever pattern it deploys.
const COMMON_REQUIRED_FIELDS: [&str; 2] = ["KeyPairName", "VpcId"];

/// The quorum mechanisms a clustered pattern offers, and the one that needs no witness
/// instance of its own. EMULATOR-owned vocabulary (see [`sap_hana_single_fields`]).
const CLUSTER_QUORUM_TYPES: [&str; 2] = ["FileShareWitness", "CloudWitness"];
/// See [`CLUSTER_QUORUM_TYPES`].
const CLUSTER_QUORUM_CLOUD_WITNESS: &str = "CloudWitness";

/// The Microsoft SQL Server versions and editions the seeded SQL Server patterns offer,
/// and the Microsoft Active Directory editions the AD patterns offer. EMULATOR-owned
/// vocabularies (see [`sap_hana_single_fields`]).
const SQL_SERVER_VERSIONS: [&str; 3] = ["2017", "2019", "2022"];
/// See [`SQL_SERVER_VERSIONS`].
const SQL_SERVER_EDITIONS: [&str; 2] = ["Standard", "Enterprise"];
/// See [`SQL_SERVER_VERSIONS`].
const DIRECTORY_EDITIONS: [&str; 2] = ["Standard", "Enterprise"];
/// How many nodes a scale-out or multi-controller pattern may launch. EMULATOR-owned,
/// see [`sap_hana_single_fields`].
const NODE_COUNTS: [&str; 3] = ["2", "3", "4"];

/// The infrastructure fields EVERY seeded deployment pattern reads.
///
/// The names are the infrastructure keys the model's own `CreateDeployment` example
/// submits (`KeyPairName`, `VpcId`, `CreateSecurityGroup`,
/// `DisableDeploymentRollback`, `SaveDeploymentArtifacts`, `SnsTopicArn`), and each one
/// is a valid `DeploymentSpecifications` key (len 3..256, `^[a-zA-Z0-9-:]+$`), so a
/// caller can take a reported name straight to `CreateDeployment`.
fn common_specification_fields() -> Vec<SpecificationField> {
    vec![
        spec_field(
            "KeyPairName",
            "The name of the EC2 key pair used to access the instances this deployment \
             launches.",
            &[],
        ),
        spec_field(
            "VpcId",
            "The id of the VPC the deployment launches its resources into.",
            &[],
        ),
        spec_field(
            FIELD_CREATE_SECURITY_GROUP,
            "Whether the deployment creates a security group for the instances it launches.",
            &[REQUIRED_YES, REQUIRED_NO],
        ),
        conditional_field(
            "SecurityGroupId",
            "The id of an existing security group to attach to the instances this \
             deployment launches.",
            &[],
            // Read only when the deployment is told NOT to create a security group,
            // which is what a conditional on another field expresses.
            vec![conditional(
                FIELD_CREATE_SECURITY_GROUP,
                COMPARATOR_EQUAL,
                REQUIRED_NO,
            )],
        ),
        spec_field(
            "DisableDeploymentRollback",
            "Whether the deployment keeps the resources it created when a step fails.",
            &[REQUIRED_YES, REQUIRED_NO],
        ),
        spec_field(
            "SaveDeploymentArtifacts",
            "Whether the deployment keeps the artifacts it generated after it finishes.",
            &[REQUIRED_YES, REQUIRED_NO],
        ),
        spec_field(
            "SnsTopicArn",
            "The ARN of the Amazon SNS topic the deployment publishes its notifications \
             to.",
            &[],
        ),
    ]
}

/// The fields every pattern of `workload` reads on top of
/// [`common_specification_fields`]: the settings that belong to the application being
/// deployed rather than to one way of deploying it.
///
/// SAP's names are the SAP-specific keys the model's own `CreateDeployment` example
/// submits (`SapSysGroupId`, `SapVirtualIPOptIn`, `EnableEbsVolumeEncryption`,
/// `ProxyServerAddress`, `Timezone`); the rest are the EMULATOR's own, for the reason
/// [`sap_hana_single_fields`] gives.
fn workload_specification_fields(workload: &str) -> Vec<SpecificationField> {
    match workload {
        "SAP" => vec![
            spec_field(
                "SapSysGroupId",
                "The group id of the SAP system's sapsys group.",
                &[],
            ),
            spec_field(
                "SapVirtualIPOptIn",
                "Whether the SAP instances are reached through a virtual IP address.",
                &[REQUIRED_YES, REQUIRED_NO],
            ),
            spec_field(
                FIELD_ENABLE_EBS_VOLUME_ENCRYPTION,
                "Whether the EBS volumes of the SAP instances are encrypted.",
                &[REQUIRED_YES, REQUIRED_NO],
            ),
            conditional_field(
                "EbsEncryptionKeyArn",
                "The ARN of the KMS key the EBS volumes of the SAP instances are \
                 encrypted with.",
                &[],
                vec![conditional(
                    FIELD_ENABLE_EBS_VOLUME_ENCRYPTION,
                    COMPARATOR_EQUAL,
                    REQUIRED_YES,
                )],
            ),
            spec_field(
                "ProxyServerAddress",
                "The address of the proxy server the SAP instances reach the internet \
                 through.",
                &[],
            ),
            spec_field(
                "Timezone",
                "The time zone the SAP instances are configured with.",
                &[],
            ),
        ],
        "SQLServer" => vec![
            spec_field(
                "SqlServerVersion",
                "The version of Microsoft SQL Server the deployment installs.",
                &SQL_SERVER_VERSIONS,
            ),
            spec_field(
                "SqlServerEdition",
                "The edition of Microsoft SQL Server the deployment installs.",
                &SQL_SERVER_EDITIONS,
            ),
        ],
        "ActiveDirectory" => vec![
            spec_field(
                "DomainDNSName",
                "The fully qualified name of the Active Directory domain.",
                &[],
            ),
            spec_field(
                "DomainNetBIOSName",
                "The NetBIOS name of the Active Directory domain.",
                &[],
            ),
            spec_field(
                "DomainAdminUser",
                "The name of the account the deployment administers the domain with.",
                &[],
            ),
        ],
        _ => Vec::new(),
    }
}

/// The fields a pattern that deploys into ONE Availability Zone reads.
fn one_zone_fields() -> Vec<SpecificationField> {
    vec![
        spec_field(
            "AvailabilityZone1",
            "The Availability Zone the deployment launches its instances into.",
            &[],
        ),
        spec_field(
            "PrivateSubnet1Id",
            "The id of the private subnet, in the first Availability Zone, the \
             deployment launches its instances into.",
            &[],
        ),
    ]
}

/// The fields a pattern that spreads across TWO Availability Zones reads: the first
/// zone's, plus the second zone's — the settings a single-zone pattern has no use for.
fn two_zone_fields() -> Vec<SpecificationField> {
    let mut fields = one_zone_fields();
    fields.extend([
        spec_field(
            "AvailabilityZone2",
            "The second Availability Zone the deployment spreads its instances across.",
            &[],
        ),
        spec_field(
            "PrivateSubnet2Id",
            "The id of the private subnet, in the second Availability Zone, the \
             deployment launches its instances into.",
            &[],
        ),
    ]);
    fields
}

/// The fields every seeded pattern that installs an SAP HANA database reads.
fn hana_database_fields() -> Vec<SpecificationField> {
    vec![
        spec_field(
            "HanaSid",
            "The system id of the SAP HANA database the deployment installs.",
            &[],
        ),
        spec_field(
            "HanaInstanceType",
            "The EC2 instance type of the SAP HANA database instances.",
            &[],
        ),
        spec_field(
            "HanaInstanceNumber",
            "The instance number of the SAP HANA database.",
            &[],
        ),
    ]
}

/// The fields ONE seeded pattern reads of its OWN, on top of the common and per-workload
/// ones: the settings that belong to this way of deploying the application rather than to
/// the application. Each of these is named by its pattern's row in the catalogue table
/// ([`seeded_workloads`]), so what a pattern publishes lives beside its name instead of
/// in a second list keyed by it.
///
/// This is what makes two patterns of one workload publish different schemas — a pattern
/// that spreads across two Availability Zones reads the second zone's settings and the
/// cluster's, which a single-instance pattern of the same workload has no use for.
///
/// Launch Wizard's catalogue is service-owned (no API creates a workload, a pattern or a
/// specification), so these names and vocabularies are the EMULATOR's own for the
/// patterns it seeds — like [`CATALOGUE_PATTERN_VERSIONS`] — rather than a claim about
/// which settings AWS publishes for them; the ones the model's own `CreateDeployment`
/// example submits are named where they are used. Every name is a valid
/// `DeploymentSpecifications` key, so a reported name can be taken straight to
/// `CreateDeployment`.
fn sap_hana_single_fields() -> Vec<SpecificationField> {
    let mut fields = hana_database_fields();
    fields.extend(one_zone_fields());
    fields
}

/// The scale-out SAP HANA pattern's own fields: the single-instance pattern's, plus how
/// far the database is spread and what the nodes share. See [`sap_hana_single_fields`].
fn sap_hana_multi_node_fields() -> Vec<SpecificationField> {
    let mut fields = sap_hana_single_fields();
    fields.extend([
        spec_field(
            "HanaWorkerNodeCount",
            "How many worker nodes the scale-out SAP HANA database is spread over.",
            &NODE_COUNTS,
        ),
        spec_field(
            "HanaSharedFileSystemId",
            "The id of the shared file system the SAP HANA nodes mount.",
            &[],
        ),
    ]);
    fields
}

/// The highly available SAP NetWeaver on SAP HANA pattern's own fields: a HANA database
/// across TWO zones, plus the NetWeaver system in front of it. See
/// [`sap_hana_single_fields`].
fn sap_nw_on_hana_ha_fields() -> Vec<SpecificationField> {
    let mut fields = hana_database_fields();
    fields.extend(two_zone_fields());
    fields.extend([
        spec_field(
            "SapNWSid",
            "The system id of the SAP NetWeaver system the deployment installs.",
            &[],
        ),
        spec_field(
            "SapNWInstanceType",
            "The EC2 instance type of the SAP NetWeaver instances.",
            &[],
        ),
        spec_field(
            "EnableHanaSystemReplication",
            "Whether the SAP HANA database replicates to the second Availability Zone.",
            &[REQUIRED_YES, REQUIRED_NO],
        ),
    ]);
    fields
}

/// The single-instance Microsoft SQL Server pattern's own fields. See
/// [`sap_hana_single_fields`].
fn sql_server_single_fields() -> Vec<SpecificationField> {
    let mut fields = one_zone_fields();
    fields.extend([
        spec_field(
            "SqlServerInstanceType",
            "The EC2 instance type of the Microsoft SQL Server instance.",
            &[],
        ),
        spec_field(
            "SqlServerDataVolumeSize",
            "The size, in gibibytes, of the volume holding the Microsoft SQL Server data \
             files.",
            &[],
        ),
    ]);
    fields
}

/// The highly available Microsoft SQL Server pattern's own fields: two zones, the
/// availability group, how the cluster establishes quorum and the domain its nodes join.
/// See [`sap_hana_single_fields`].
fn sql_server_ha_fields() -> Vec<SpecificationField> {
    let mut fields = two_zone_fields();
    fields.extend([
        spec_field(
            "SqlServerInstanceType",
            "The EC2 instance type of the Microsoft SQL Server instances.",
            &[],
        ),
        spec_field(
            "AlwaysOnAvailabilityGroupName",
            "The name of the Always On availability group the deployment creates.",
            &[],
        ),
        spec_field(
            FIELD_CLUSTER_QUORUM_TYPE,
            "How the Windows Server failover cluster establishes quorum.",
            &CLUSTER_QUORUM_TYPES,
        ),
        conditional_field(
            "WitnessInstanceType",
            "The EC2 instance type of the instance hosting the cluster's file share \
             witness.",
            &[],
            vec![conditional(
                FIELD_CLUSTER_QUORUM_TYPE,
                COMPARATOR_NOT_EQUAL,
                CLUSTER_QUORUM_CLOUD_WITNESS,
            )],
        ),
        spec_field(
            "DomainDNSName",
            "The fully qualified name of the Active Directory domain the cluster nodes \
             join.",
            &[],
        ),
    ]);
    fields
}

/// The AWS Directory Service managed directory pattern's own fields: the two zones a
/// managed directory is spread over and its edition. See [`sap_hana_single_fields`].
fn aws_managed_ad_fields() -> Vec<SpecificationField> {
    let mut fields = two_zone_fields();
    fields.push(spec_field(
        "DirectoryEdition",
        "The edition of the AWS Directory Service managed directory the deployment \
         creates.",
        &DIRECTORY_EDITIONS,
    ));
    fields
}

/// The self-managed Active Directory pattern's own fields: the domain controllers the
/// caller administers, across two zones. See [`sap_hana_single_fields`].
fn self_managed_ad_fields() -> Vec<SpecificationField> {
    let mut fields = two_zone_fields();
    fields.extend([
        spec_field(
            "DomainControllerInstanceType",
            "The EC2 instance type of the domain controllers the deployment launches.",
            &[],
        ),
        spec_field(
            "DomainControllerCount",
            "How many domain controllers the deployment launches.",
            &NODE_COUNTS,
        ),
    ]);
    fields
}

/// What one pattern marks `required` beyond [`COMMON_REQUIRED_FIELDS`]: the settings a
/// deployment of THAT pattern cannot be created without, named by its row in the
/// catalogue table ([`seeded_workloads`]) beside the fields it publishes.
///
/// Requirement is the pattern's own statement about a field (see [`spec_field`]): a
/// pattern that spreads across Availability Zones or forms a cluster cannot make those
/// choices for the caller — which zone, which subnet, which quorum, which domain — so it
/// insists on them, while a single-instance pattern launches into whichever zone and
/// subnet of the named VPC the service picks and defaults the database identifiers, so
/// the caller only has to name the key pair and the VPC. That is also why the request the
/// model documents for the single-instance SAP HANA pattern carries nothing beyond the
/// common core and is served as written.
const SAP_HANA_SINGLE_REQUIRED: &[&str] = &[];
/// See [`SAP_HANA_SINGLE_REQUIRED`].
const SAP_HANA_MULTI_NODE_REQUIRED: &[&str] = &[
    "HanaSid",
    "HanaInstanceType",
    "AvailabilityZone1",
    "PrivateSubnet1Id",
    "HanaWorkerNodeCount",
];
/// See [`SAP_HANA_SINGLE_REQUIRED`].
const SAP_NW_ON_HANA_HA_REQUIRED: &[&str] = &[
    "HanaSid",
    "HanaInstanceType",
    "AvailabilityZone1",
    "PrivateSubnet1Id",
    "AvailabilityZone2",
    "PrivateSubnet2Id",
    "SapNWSid",
    "SapNWInstanceType",
    "EnableHanaSystemReplication",
];
/// See [`SAP_HANA_SINGLE_REQUIRED`].
const SQL_SERVER_SINGLE_REQUIRED: &[&str] = &[
    "SqlServerVersion",
    "SqlServerInstanceType",
    "AvailabilityZone1",
    "PrivateSubnet1Id",
];
/// See [`SAP_HANA_SINGLE_REQUIRED`].
const SQL_SERVER_HA_REQUIRED: &[&str] = &[
    "SqlServerVersion",
    "SqlServerInstanceType",
    "AvailabilityZone1",
    "PrivateSubnet1Id",
    "AvailabilityZone2",
    "PrivateSubnet2Id",
    "AlwaysOnAvailabilityGroupName",
    FIELD_CLUSTER_QUORUM_TYPE,
    "DomainDNSName",
];
/// See [`SAP_HANA_SINGLE_REQUIRED`].
const AWS_MANAGED_AD_REQUIRED: &[&str] = &[
    "DomainDNSName",
    "DirectoryEdition",
    "AvailabilityZone1",
    "PrivateSubnet1Id",
    "AvailabilityZone2",
    "PrivateSubnet2Id",
];
/// See [`SAP_HANA_SINGLE_REQUIRED`].
const SELF_MANAGED_AD_REQUIRED: &[&str] = &[
    "DomainDNSName",
    "DomainNetBIOSName",
    "DomainAdminUser",
    "DomainControllerInstanceType",
    "AvailabilityZone1",
    "PrivateSubnet1Id",
    "AvailabilityZone2",
    "PrivateSubnet2Id",
];

/// Apply one pattern's required set over its composed schema.
///
/// Requirement is stated per pattern rather than per field, so this is where the two
/// meet: a field the pattern names is published as [`REQUIRED_YES`], every other field
/// as [`REQUIRED_NO`]. A required name the pattern does not publish could not be filled
/// in by any caller, so it is a catalogue bug rather than a schema — caught here in the
/// test build instead of leaving a pattern no `CreateDeployment` can satisfy.
fn mark_required(fields: &mut [SpecificationField], required: &[&str]) {
    for field in fields.iter_mut() {
        field.required = required.contains(&field.name.as_str());
    }
    debug_assert!(
        required
            .iter()
            .all(|name| fields.iter().any(|field| field.name == *name)),
        "every required specification must be one the pattern publishes"
    );
}

/// The specification schema the seeded catalogue publishes for ONE deployment pattern:
/// the infrastructure fields every pattern reads, the fields its workload adds, the
/// fields the pattern itself defines, and then the pattern's own required set — all read
/// off that pattern's catalogue row.
///
/// Composed per pattern rather than per workload, because the schema a pattern publishes
/// is the contract a `CreateDeployment` for THAT pattern fills in — which is why
/// `required`, `allowedValues` and `conditionals` are modelled per field and per
/// pattern. Well inside `DeploymentSpecificationsData`'s `@length(min: 1, max: 100)`.
///
/// A field NAME is what a caller fills in, so each one is published once: a name two of
/// the three layers state (a setting that belongs to the application and that one way of
/// deploying it also reads) keeps the first layer's definition rather than being reported
/// twice, which would publish two contradictory contracts for one key.
fn seeded_specification_fields(workload: &str, pattern: &SeededPattern) -> Vec<SpecificationField> {
    let mut fields = common_specification_fields();
    fields.extend(workload_specification_fields(workload));
    fields.extend((pattern.own_fields)());
    let mut published = BTreeSet::new();
    fields.retain(|field| published.insert(field.name.clone()));
    let required: Vec<&str> = COMMON_REQUIRED_FIELDS
        .iter()
        .copied()
        .chain(pattern.own_required.iter().copied())
        .collect();
    mark_required(&mut fields, &required);
    fields
}

/// One deployment pattern of the seeded catalogue, as the table in
/// [`seeded_workloads`] spells it: its `DeploymentPatternName`, its display name, its
/// description, the fields it reads of its own and the names it marks required beyond
/// [`COMMON_REQUIRED_FIELDS`].
///
/// Everything the catalogue states about a pattern is in ONE row, so a pattern cannot be
/// seeded with the schema or the required set of another, and one added here cannot be
/// forgotten in a second list keyed by its name.
struct SeededPattern {
    /// `DeploymentPatternName`: len 1..256, `^[A-Za-z0-9][a-zA-Z0-9-]*$`.
    name: &'static str,
    display_name: &'static str,
    description: &'static str,
    /// See [`sap_hana_single_fields`].
    own_fields: fn() -> Vec<SpecificationField>,
    /// See [`SAP_HANA_SINGLE_REQUIRED`].
    own_required: &'static [&'static str],
}

/// One workload of the seeded catalogue: its `WorkloadName`, its display name, the
/// description `GetWorkload` reports and the patterns it offers.
type SeededWorkload = (
    &'static str,
    &'static str,
    &'static str,
    &'static [SeededPattern],
);

/// The workload catalogue the emulator seeds at first touch.
///
/// Launch Wizard's catalogue is service-owned (there is no API to create a workload or
/// a deployment pattern), so the emulator seeds it. The entries are the Launch Wizard
/// applications AWS documents — SAP, Microsoft SQL Server and Microsoft Active
/// Directory — with the deployment patterns each of those applications offers, the
/// specification schema each pattern accepts, and the one version of each. The model
/// constrains the shapes of these values: `WorkloadName` is `^[A-Za-z][a-zA-Z0-9-_]*$`
/// (len 1..100) and `DeploymentPatternName` is `^[A-Za-z0-9][a-zA-Z0-9-]*$` (len
/// 1..256).
///
/// `description` is the per-workload metadata `WorkloadData` (and therefore
/// `GetWorkload`) reports: what the entry's application deploys; each pattern carries
/// its own display name, description, specification schema and version.
fn seeded_workloads() -> BTreeMap<String, Workload> {
    let catalogue: [SeededWorkload; 3] = [
        (
            "SAP",
            "SAP",
            "Deploy SAP HANA and SAP NetWeaver applications on Amazon Web Services.",
            &[
                SeededPattern {
                    name: "SapHanaSingle",
                    display_name: "SAP HANA on a single instance",
                    description: "Deploys an SAP HANA database on one instance.",
                    own_fields: sap_hana_single_fields,
                    own_required: SAP_HANA_SINGLE_REQUIRED,
                },
                SeededPattern {
                    name: "SapHanaMultiNode",
                    display_name: "SAP HANA on multiple nodes",
                    description: "Deploys a scale-out SAP HANA database across several \
                                  instances.",
                    own_fields: sap_hana_multi_node_fields,
                    own_required: SAP_HANA_MULTI_NODE_REQUIRED,
                },
                SeededPattern {
                    name: "SapNWOnHanaHA",
                    display_name: "SAP NetWeaver on SAP HANA, highly available",
                    description: "Deploys SAP NetWeaver on an SAP HANA database across two \
                                  Availability Zones.",
                    own_fields: sap_nw_on_hana_ha_fields,
                    own_required: SAP_NW_ON_HANA_HA_REQUIRED,
                },
            ][..],
        ),
        (
            "SQLServer",
            "Microsoft SQL Server",
            "Deploy Microsoft SQL Server, standalone or highly available, on Amazon Web Services.",
            &[
                SeededPattern {
                    name: "SqlServerSingle",
                    display_name: "Microsoft SQL Server on a single instance",
                    description: "Deploys Microsoft SQL Server on one instance.",
                    own_fields: sql_server_single_fields,
                    own_required: SQL_SERVER_SINGLE_REQUIRED,
                },
                SeededPattern {
                    name: "SqlServerHA",
                    display_name: "Microsoft SQL Server, highly available",
                    description: "Deploys a Microsoft SQL Server Always On availability group \
                                  across two Availability Zones.",
                    own_fields: sql_server_ha_fields,
                    own_required: SQL_SERVER_HA_REQUIRED,
                },
            ][..],
        ),
        (
            "ActiveDirectory",
            "Microsoft Active Directory",
            "Deploy AWS Managed Microsoft AD or self-managed Active Directory on Amazon Web \
             Services.",
            &[
                SeededPattern {
                    name: "AWSManagedAD",
                    display_name: "AWS Managed Microsoft AD",
                    description: "Deploys an AWS Directory Service managed Microsoft AD \
                                  directory.",
                    own_fields: aws_managed_ad_fields,
                    own_required: AWS_MANAGED_AD_REQUIRED,
                },
                SeededPattern {
                    name: "SelfManagedAD",
                    display_name: "Self-managed Active Directory",
                    description: "Deploys Active Directory domain controllers you administer \
                                  yourself.",
                    own_fields: self_managed_ad_fields,
                    own_required: SELF_MANAGED_AD_REQUIRED,
                },
            ][..],
        ),
    ];
    catalogue
        .into_iter()
        .map(|(workload_name, display_name, description, patterns)| {
            let patterns = patterns
                .iter()
                .map(|pattern| {
                    (
                        pattern.name.to_string(),
                        DeploymentPattern {
                            workload_name: workload_name.to_string(),
                            pattern_name: pattern.name.to_string(),
                            display_name: pattern.display_name.to_string(),
                            description: pattern.description.to_string(),
                            status: PATTERN_ACTIVE.to_string(),
                            status_message: None,
                            workload_version_name: CATALOGUE_VERSION.to_string(),
                            versions: seeded_pattern_versions(pattern.description),
                            specifications: seeded_specification_fields(workload_name, pattern),
                        },
                    )
                })
                .collect();
            (
                workload_name.to_string(),
                Workload {
                    workload_name: workload_name.to_string(),
                    display_name: display_name.to_string(),
                    description: description.to_string(),
                    status: WORKLOAD_ACTIVE.to_string(),
                    patterns,
                },
            )
        })
        .collect()
}

/// The versions the catalogue publishes for one seeded deployment pattern, oldest
/// first: one per entry of [`CATALOGUE_PATTERN_VERSIONS`].
///
/// A version's description is DERIVED from the pattern's own description and the
/// version's name, so it says what that version of that pattern deploys without
/// inventing per-version release notes this emulator does not have.
fn seeded_pattern_versions(pattern_description: &str) -> Vec<PatternVersion> {
    CATALOGUE_PATTERN_VERSIONS
        .iter()
        .map(|version_name| PatternVersion {
            version_name: (*version_name).to_string(),
            description: format!(
                "Version {version_name} of this deployment pattern. {pattern_description}"
            ),
        })
        .collect()
}

/// The region to build a new resource's ARN in: the request's SigV4 credential-scope
/// region, or [`DEFAULT_REGION`] when the request is unsigned.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a deployment: `arn:aws:launchwizard:<region>:<account>:deployment/<id>`.
pub fn deployment_arn(region: &str, deployment_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("deployment/{deployment_id}"))
}

/// The resource-type segment of a deployment ARN, as [`deployment_arn`] builds it.
const DEPLOYMENT_RESOURCE_PREFIX: &str = "deployment/";

/// True when `id` has the form a Launch Wizard `deploymentId` takes: the canonical
/// hyphenated UUID [`ComAmazonawsLaunchwizardLaunchwizardState::mint_deployment_id`]
/// mints — five hex groups laid out 8-4-4-4-12. Only that hyphenated form is accepted;
/// the bare 32-hex form a UUID parser also takes is not a deployment id.
pub fn is_deployment_id(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == UUID_GROUP_LENGTHS.len()
        && groups
            .iter()
            .zip(UUID_GROUP_LENGTHS)
            .all(|(group, length)| {
                group.len() == *length && group.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
}

/// The 8-4-4-4-12 grouping of a UUID's canonical text form (RFC 4122 §3), which is both
/// what [`fresh_uuid_v4`] writes and what [`is_deployment_id`] accepts.
const UUID_GROUP_LENGTHS: &[usize] = &[8, 4, 4, 4, 12];

/// A fresh identifier in the canonical text form of a version-4 UUID.
///
/// Derived from a digest rather than from a UUID crate, the way the sibling services
/// mint their UUID-shaped ids (`service-com-amazonaws-kafka-kafka`'s
/// `uuid_v4_shaped_from_digest`, `service-com-amazonaws-codestarnotifications-…`): the
/// seed is this process's own draw counter and the wall clock, neither of them anything
/// a caller sends, so the id is unique per draw and derived from no request value. A
/// digest keeps `Cargo.lock` — the file every concurrent change also edits — untouched,
/// which is why no new crate is pulled in for 16 bytes.
fn fresh_uuid_v4() -> String {
    use ::sha2::Digest as _;
    use std::fmt::Write as _;

    /// Draws so far in this process. Makes two draws within one clock tick distinct.
    static DRAWS: ::std::sync::atomic::AtomicU64 = ::std::sync::atomic::AtomicU64::new(0);

    let draw = DRAWS.fetch_add(1, ::std::sync::atomic::Ordering::Relaxed);
    let since_epoch = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    let mut hasher = ::sha2::Sha256::new();
    hasher.update(ARN_SERVICE.as_bytes());
    hasher.update(draw.to_be_bytes());
    hasher.update(since_epoch.as_nanos().to_be_bytes());
    let digest = hasher.finalize();

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    // RFC 4122 §4.4: the version goes in the high nibble of byte 6, the variant in the
    // two high bits of byte 8, so the text form is a well-formed version-4 UUID.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut rendered = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        // The hyphens of the 8-4-4-4-12 grouping fall after bytes 4, 6, 8 and 10.
        if matches!(index, 4 | 6 | 8 | 10) {
            rendered.push('-');
        }
        // Infallible: writing to a `String` only fails if the formatter does, and
        // `{:02x}` cannot.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// The `deploymentId` a tagging-API `resourceArn` names, or `None` when the ARN is
/// not one that can name a Launch Wizard deployment at all.
///
/// The tagging operations take their resource as an ARN, so the ARN is READ before
/// anything is looked up: it has to be `arn:<partition>:launchwizard:<region>:
/// <account>:deployment/<deploymentId>` with every field present and the id in the
/// deployment-id form ([`is_deployment_id`]). Anything else — a different service or
/// resource type, a missing field, an id that is not a deployment id — is a resource
/// ARN the service cannot parse, which is a malformed request rather than a resource
/// that is merely absent. The deployment id is what identifies the deployment, so the
/// partition, region and account the ARN carries are the scope this emulator serves
/// and are not a second key.
pub fn deployment_id_in_arn(arn: &str) -> Option<&str> {
    // `splitn(6, ..)` keeps the resource field whole: only the first five colons are
    // structural, and a resource field may contain more. The fields are destructured
    // straight off the iterator, so this request-path parse allocates nothing.
    let mut fields = arn.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(ARN_SERVICE),
        Some(region),
        Some(account),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    if partition.is_empty() || region.is_empty() || account.is_empty() {
        return None;
    }
    let id = resource.strip_prefix(DEPLOYMENT_RESOURCE_PREFIX)?;
    is_deployment_id(id).then_some(id)
}

/// The wire error code the tagging APIs answer for a resource that does not exist.
///
/// This is the shared tagging layer's own not-found code, which this service's model
/// does not declare (it declares the operation-specific `ResourceNotFoundException`
/// the deployment reads raise), so it is answered through
/// [`crate::scaffolded::Scaffolded::unmodeled`].
pub const TAGGING_NOT_FOUND_CODE: &str = "NotFoundException";

/// The wire error code the tagging APIs answer for a `resourceArn` they cannot parse.
/// Unmodeled for the same reason as [`TAGGING_NOT_FOUND_CODE`].
pub const TAGGING_BAD_REQUEST_CODE: &str = "BadRequestException";

/// The tagging layer's "this resource does not exist" refusal: HTTP 404
/// `NotFoundException`.
///
/// Every tagging operation answers it the same way, and it DOMINATES the shape of the
/// tags the caller sent: a tag map or key list is only judged once the resource it
/// would be written to is known to exist.
pub fn tagging_resource_not_found<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(TAGGING_NOT_FOUND_CODE, 404, "Resource was not found")
}

/// The tagging layer's "this is not a resource ARN" refusal: HTTP 400
/// `BadRequestException`. Answered BEFORE any lookup — an ARN that cannot be parsed
/// names no resource whose existence could be decided.
pub fn tagging_invalid_resource_arn<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::unmodeled(
        TAGGING_BAD_REQUEST_CODE,
        400,
        "Invalid input resource arn",
    )
}

/// The modelled `TagKey` rules, from
/// `smithy_query(query="shape", shape="Tags,TagKeyList")`: a `Tags` key (and a
/// `TagKeyList` member) targets `TagKey`, which is `@length(min: 1, max: 128)` with
/// `@pattern("^[a-zA-Z0-9 _.:/=+@-]+$")`.
///
/// Restated here only because codegen captures a collection's element/value
/// constraints but not a map's KEY constraints (see [`TAG_KEY_SCHEMA`]). The
/// restatement is pinned to the model by
/// `tests/all/resource_tagging.rs::the_tag_rules_the_handlers_enforce_are_the_models`,
/// which reads these three values out of `smithy-model/` and fails if they drift.
pub const TAG_KEY_MIN_LEN: i64 = 1;
/// See [`TAG_KEY_MIN_LEN`].
pub const TAG_KEY_MAX_LEN: i64 = 128;
/// See [`TAG_KEY_MIN_LEN`].
pub const TAG_KEY_PATTERN: &str = "^[a-zA-Z0-9 _.:/=+@-]+$";

/// The modelled `DeploymentSpecifications` KEY rules, from
/// `smithy_query(query="shape", shape="DeploymentSpecifications,KeyString")`: a
/// `DeploymentSpecifications` key targets `KeyString`, which is
/// `@length(min: 3, max: 256)` with `@pattern("^[a-zA-Z0-9-:]+$")`.
///
/// Restated here for the same reason as [`TAG_KEY_MIN_LEN`] — codegen does not carry a
/// map's KEY constraints — and pinned to the model by
/// `tests/all/create_deployment_validation.rs::the_specification_key_rules_the_handlers_enforce_are_the_models`.
pub const SPECIFICATION_KEY_MIN_LEN: i64 = 3;
/// See [`SPECIFICATION_KEY_MIN_LEN`].
pub const SPECIFICATION_KEY_MAX_LEN: i64 = 256;
/// See [`SPECIFICATION_KEY_MIN_LEN`].
pub const SPECIFICATION_KEY_PATTERN: &str = "^[a-zA-Z0-9-:]+$";

/// A map KEY leaf schema: one string member carrying the key shape's `@length` and
/// `@pattern`.
///
/// A map's KEY constraints are the one part of a shape the generated schemas do not
/// carry (codegen captures a collection's element/value constraints only), so a map key
/// is the one thing the constraint walk over an input cannot judge — it judges a key
/// against the member's schema, which composes the VALUE shape's constraints. These
/// schemas give the key rules back, so the rules a key is judged by are the model's and
/// the refusal is worded by the same renderer as every other constraint. `member` is
/// the INPUT member the map arrived on, so the refusal names what the caller sent.
fn map_key_schema(
    shape_id: &'static str,
    member: &'static str,
    traits: &'static ::std::sync::LazyLock<::aws_smithy_schema::TraitMap>,
) -> ::aws_smithy_schema::Schema {
    ::aws_smithy_schema::Schema::new_member(
        ::aws_smithy_schema::ShapeId::from_static(shape_id, "com.amazonaws.launchwizard", member),
        ::aws_smithy_schema::ShapeType::String,
        member,
        0,
    )
    .with_traits(traits)
}

/// A key shape's `@length` and `@pattern` as the trait map the framework's constraint
/// walk reads. Each key shape holds its own in a static, because
/// [`map_key_schema`] composes a `&'static` one.
fn map_key_traits(
    min_len: i64,
    max_len: i64,
    pattern: &'static str,
) -> ::aws_smithy_schema::TraitMap {
    let mut traits = ::aws_smithy_schema::TraitMap::new();
    traits.insert(Box::new(
        ::aws_smithy_schema_server::constraints::LengthTrait::new(Some(min_len), Some(max_len)),
    ));
    traits.insert(Box::new(
        ::aws_smithy_schema_server::constraints::PatternTrait::new(pattern),
    ));
    traits
}

/// [`TAG_KEY_MIN_LEN`]/[`TAG_KEY_MAX_LEN`]/[`TAG_KEY_PATTERN`] as the walk reads them.
static TAG_KEY_TRAITS: ::std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    ::std::sync::LazyLock::new(|| {
        map_key_traits(TAG_KEY_MIN_LEN, TAG_KEY_MAX_LEN, TAG_KEY_PATTERN)
    });

/// [`SPECIFICATION_KEY_MIN_LEN`]/[`SPECIFICATION_KEY_MAX_LEN`]/
/// [`SPECIFICATION_KEY_PATTERN`] as the walk reads them.
static SPECIFICATION_KEY_TRAITS: ::std::sync::LazyLock<::aws_smithy_schema::TraitMap> =
    ::std::sync::LazyLock::new(|| {
        map_key_traits(
            SPECIFICATION_KEY_MIN_LEN,
            SPECIFICATION_KEY_MAX_LEN,
            SPECIFICATION_KEY_PATTERN,
        )
    });

/// The key leaf of a `Tags` map, as it arrives on the `tags` member. See
/// [`map_key_schema`].
static TAG_KEY_SCHEMA: ::std::sync::LazyLock<::aws_smithy_schema::Schema> =
    ::std::sync::LazyLock::new(|| {
        map_key_schema(
            "com.amazonaws.launchwizard#Tags$key",
            "tags",
            &TAG_KEY_TRAITS,
        )
    });

/// The key leaf of a `DeploymentSpecifications` map, as it arrives on the
/// `specifications` member. See [`map_key_schema`].
static SPECIFICATION_KEY_SCHEMA: ::std::sync::LazyLock<::aws_smithy_schema::Schema> =
    ::std::sync::LazyLock::new(|| {
        map_key_schema(
            "com.amazonaws.launchwizard#DeploymentSpecifications$key",
            "specifications",
            &SPECIFICATION_KEY_TRAITS,
        )
    });

/// The refusal message for the map keys that break the shape `schema` carries, or
/// `None` when every one of them is within it.
///
/// Worded by [`validation_report`], the same renderer `src/service.rs` words a
/// boundary refusal with, so a key refused inside a handler reads exactly like one
/// refused at the boundary.
fn map_key_refusal<'a>(
    schema: &::aws_smithy_schema::Schema,
    keys: impl Iterator<Item = &'a str>,
) -> Option<String> {
    use ::aws_smithy_schema::serde::ShapeSerializer as _;

    let mut validator = ::aws_smithy_schema_server::constraints::ValidatingSerializer::new();
    for key in keys {
        // Infallible: the validator accumulates violations instead of failing.
        let _ = validator.write_string(schema, key);
    }
    let violations = validator.into_violations();
    (!violations.is_empty()).then(|| validation_report(&violations))
}

/// The refusal message for the `Tags` keys that break their modelled shape, or `None`
/// when every one of them is within it. See [`TAG_KEY_SCHEMA`].
pub fn tag_key_refusal<'a>(keys: impl Iterator<Item = &'a str>) -> Option<String> {
    map_key_refusal(&TAG_KEY_SCHEMA, keys)
}

/// The refusal message for the `DeploymentSpecifications` keys that break their
/// modelled shape, or `None` when every one of them is within it. See
/// [`SPECIFICATION_KEY_SCHEMA`].
pub fn specification_key_refusal<'a>(keys: impl Iterator<Item = &'a str>) -> Option<String> {
    map_key_refusal(&SPECIFICATION_KEY_SCHEMA, keys)
}

/// The refusal message for a request whose own modelled constraints a handler judges
/// itself, or `None` when it satisfies every one of them.
///
/// The tagging operations keep their collection out of the boundary walk so the
/// resource is decided first (see `ops/types/tag_resource_types.rs`), and then judge
/// it HERE against a twin of the input shape that keeps the constraints. Both the
/// rules and the wording are therefore the boundary's: same walk, same
/// [`validation_report`].
pub fn constraint_refusal(
    value: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    schema: &::aws_smithy_schema::Schema,
) -> Option<String> {
    let violations = ::aws_smithy_schema_server::constraints::validate(value, schema);
    (!violations.is_empty()).then(|| validation_report(&violations))
}

/// The validation warnings a submitted `specifications` map raises against the
/// specification schema of the deployment pattern it is submitted for — the SAME
/// schema `GetWorkloadDeploymentPattern` reports for that pattern.
///
/// Launch Wizard judges a deployment's settings against the schema of the pattern it
/// deploys, and `UpdateDeployment$force` exists to apply an update "even if validation
/// warnings are present" — so these are the warnings that member overrides. A CREATE
/// models no such member, so for `CreateDeployment` the same clauses are a refusal
/// ([`specification_create_refusal`]): the schema published with `required` and
/// `allowedValues` is what a deployment of that pattern has to fill in. Each one
/// is a statement the STORED schema makes about the submitted map: a setting the
/// pattern does not define, a value outside a field's `allowedValues`, and a field the
/// pattern marks required that the map does not carry.
///
/// A field's `conditionals` are PUBLISHED metadata only: they tell a caller which other
/// setting decides whether this one is read, and no clause here is raised from them — a
/// conditional field is accepted whatever the field it depends on carries, which is why
/// no pattern marks a conditional field required.
///
/// `DeploymentSpecifications` is `@sensitive`, so a warning names the KEY it is about
/// (a caller cannot fix the request otherwise) and never the submitted VALUE; an
/// `allowedValues` warning lists the values the SCHEMA allows, which is the pattern's
/// own published vocabulary rather than anything the caller sent.
///
/// The order is the schema's own then the submitted map's (both ordered), so one
/// request always produces the same report.
pub fn specification_warnings(
    pattern: &DeploymentPattern,
    specifications: &BTreeMap<String, String>,
) -> Vec<String> {
    let mut warnings = Vec::new();
    for field in &pattern.specifications {
        match specifications.get(&field.name) {
            None if field.required => warnings.push(format!(
                "'{}' is required by deployment pattern '{}'",
                field.name, pattern.pattern_name
            )),
            None => {}
            Some(value) => {
                let allowed = &field.allowed_values;
                if !allowed.is_empty() && !allowed.iter().any(|candidate| candidate == value) {
                    warnings.push(format!(
                        "the value of '{}' is not one of the values deployment pattern '{}' \
                         allows for it [{}]",
                        field.name,
                        pattern.pattern_name,
                        allowed.join(", ")
                    ));
                }
            }
        }
    }
    for name in specifications.keys() {
        if !pattern.specifications.iter().any(|f| &f.name == name) {
            warnings.push(format!(
                "'{name}' is not a specification of deployment pattern '{}'",
                pattern.pattern_name
            ));
        }
    }
    warnings
}

/// How many clauses, then the clauses — the shape [`validation_report`] states a
/// refusal in, because a caller reads both the same way. `noun` is pluralised by
/// appending an `s`, which is what both callers' nouns take.
fn counted_clauses(noun: &str, clauses: &[String]) -> String {
    let plural = if clauses.len() == 1 {
        noun.to_string()
    } else {
        format!("{noun}s")
    };
    format!(
        "{} validation {plural} detected: {}",
        clauses.len(),
        clauses.join("; ")
    )
}

/// The `ValidationException` message an update refused for its specification warnings
/// carries: every warning, and the member that applies the update regardless.
///
/// AWS documents no text for this refusal, so only the HTTP 400 and the modelled
/// `ValidationException` code are the contract.
pub fn specification_warning_refusal(warnings: &[String]) -> String {
    format!(
        "{}; resubmit with force set to true to update the deployment anyway.",
        counted_clauses("warning", warnings)
    )
}

/// The `ValidationException` message a CREATE refused for its specification warnings
/// carries.
///
/// `CreateDeployment` models no `force`, so what an update merely warns about is the
/// end of a create: the clauses are stated as the errors they are, and the caller is
/// pointed at the operation that publishes the schema they were judged against — the
/// one `CreateDeployment`'s own docs name "to retrieve the specifications required to
/// create a deployment". Same caveat as above: only the status and the code are the
/// contract.
pub fn specification_create_refusal(warnings: &[String]) -> String {
    format!(
        "{}; the specifications a deployment pattern accepts are reported by \
         GetWorkloadDeploymentPattern.",
        counted_clauses("error", warnings)
    )
}

/// A `SystemTime` as the modelled `Timestamp` the wire carries.
pub fn to_date_time(time: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = time
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A stored map as the `HashMap` the generated shapes carry.
pub fn to_hash_map(map: &BTreeMap<String, String>) -> HashMap<String, String> {
    map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
}

/// Encode an opaque pagination token over the cursor it resumes after.
///
/// The cursor is hex-encoded so a caller cannot read a position out of it or
/// construct one: the token is only ever a value a previous page handed back.
/// `PaginationToken` is len 1..2048, which hex of a deployment id or a workload name
/// stays well inside.
pub fn encode_token(cursor: &str) -> String {
    ::hex::encode(format!("v1:{cursor}"))
}

/// The cursor an opaque token carries, or `None` when it was not issued by
/// [`encode_token`].
fn decode_token(token: &str) -> Option<String> {
    let decoded = ::hex::decode(token).ok()?;
    let text = String::from_utf8(decoded).ok()?;
    text.strip_prefix("v1:").map(ToString::to_string)
}

/// One page of `items`: the items after the one `next_token` names, capped at
/// `max_results`, plus the token for the following page when items remain.
///
/// `cursor_of` names an item's stable cursor, so a page resumes exactly where the
/// previous one ended rather than at a remembered offset. The error is the message for
/// the operation's own `ValidationException` ([`INVALID_NEXT_TOKEN_MESSAGE`]); a token
/// this service did not issue, or one naming an item the result set no longer holds, is
/// a client error rather than a silent restart from the top.
pub fn paginate<T>(
    items: Vec<T>,
    max_results: Option<i32>,
    next_token: Option<&str>,
    cursor_of: impl Fn(&T) -> String,
) -> Result<(Vec<T>, Option<String>), String> {
    let total = items.len();
    let start = match next_token {
        None => 0,
        Some(token) => {
            // A token is opaque, so a refusal says only that it is not valid: neither
            // which of the two ways it is unusable, nor the token itself.
            let cursor =
                decode_token(token).ok_or_else(|| INVALID_NEXT_TOKEN_MESSAGE.to_string())?;
            let found = items.iter().position(|item| cursor_of(item) == cursor);
            found
                .map(|index| index + 1)
                .ok_or_else(|| INVALID_NEXT_TOKEN_MESSAGE.to_string())?
        }
    };
    // `maxResults` is `@range(min: 1, max: 100)`, so the boundary validator has already
    // refused a value outside that range; absent, a page carries at most
    // `DEFAULT_PAGE_SIZE` so no single response grows with the store.
    let limit = max_results.map_or(DEFAULT_PAGE_SIZE, |max| max.max(0) as usize);
    let page: Vec<T> = items.into_iter().skip(start).take(limit).collect();
    let next = if start + page.len() < total {
        page.last().map(|item| encode_token(&cursor_of(item)))
    } else {
        None
    };
    Ok((page, next))
}

// =============================================================================
// The `ValidationException` message a refused request carries
// =============================================================================

/// The text separating the member clause of a validation clause from the requirement
/// it failed, in the framework's own wording.
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint: ";

/// How AWS states that a member's requirement is a COMPOSITION of several
/// constraints: the composed list follows this, bracketed.
const COMPOSED_MARKER: &str = "Member must satisfy constraint: ";

/// AWS's standard validation report for the modelled constraints a request failed at
/// the protocol boundary, as `src/service.rs` words this service's refusal with.
///
/// The report opens with HOW MANY members failed, then states one clause per failing
/// member: the member's dotted path and the requirement it failed. It does NOT echo
/// the rejected value — caller-supplied input is not replayed into the error channel —
/// and reports a `@pattern` as the expression itself, without the `^`/`$` the model
/// spells "whole value" with.
pub fn validation_report(violations: &[Violation]) -> String {
    let mut clauses: Vec<String> = Vec::new();
    for violation in violations {
        let clause = validation_clause(violation);
        // AWS reports one error per failing MEMBER. Two violations of the same
        // collection member render the same clause (the composed constraint set of its
        // elements), so the second is not a second error.
        if !clauses.contains(&clause) {
            clauses.push(clause);
        }
    }
    let count = clauses.len();
    let noun = if count == 1 { "error" } else { "errors" };
    format!("{count} validation {noun} detected: {}", clauses.join("; "))
}

/// One clause of [`validation_report`].
///
/// A violation found on a scalar ELEMENT of a list is reported at the COLLECTION
/// member, because the rule the element failed belongs to the collection's declared
/// element constraints; anything else is reported at the member itself.
fn validation_clause(violation: &Violation) -> String {
    collection_element_clause(violation).unwrap_or_else(|| scalar_clause(violation))
}

/// The clause for a member that failed one of its own constraints: the member and what
/// it failed, with no value echoed. An ABSENT member is still reported as the null it
/// was, which is the one "value" AWS states.
fn scalar_clause(violation: &Violation) -> String {
    let subject = if violation.value.is_none() {
        "Value null"
    } else {
        "Value"
    };
    format!(
        "{subject} at '{}' {CONSTRAINT_MARKER}{}",
        violation.path,
        requirement(violation)
    )
}

/// The clause for a scalar element of a list, or `None` when `violation` is not one.
///
/// It is reported at the collection member's path — never the element's position —
/// with the element shape's COMPLETE declared constraint set composed into one
/// requirement, whichever member of that set the element actually failed.
fn collection_element_clause(violation: &Violation) -> Option<String> {
    let collection = collection_of_element(&violation.path)?;
    Some(format!(
        "Value at '{collection}' {CONSTRAINT_MARKER}{COMPOSED_MARKER}[{}]",
        composed_requirements(collection, violation).join(", ")
    ))
}

/// The collection path a violation path's last segment is an element of, or `None`
/// when the path does not end in a collection step.
///
/// The validating walk spells a step from a list into its element
/// `<1-based-index>.member` (`aws_smithy_schema_server::constraints::path`), so
/// `filters.1.member.values.1.member` is an element of `filters.1.member.values`.
fn collection_of_element(path: &str) -> Option<&str> {
    let without_step = path.strip_suffix(".member")?;
    let (collection, index) = without_step.rsplit_once('.')?;
    let indexed = !index.is_empty() && index.chars().all(|c| c.is_ascii_digit());
    indexed.then_some(collection)
}

/// The composed constraint list AWS states for an element of `collection`: the element
/// shape's own constraints, in the order AWS lists them (length ceiling, then length
/// floor, then pattern).
///
/// A list this service's inputs declare no element constraints for falls back to the
/// one requirement the walk found, stated in the same composed form, so a constraint
/// kind the framework adds later is reported rather than dropped.
fn composed_requirements(collection: &str, violation: &Violation) -> Vec<String> {
    match element_constraints(collection) {
        Some(constraints) => {
            let mut requirements = Vec::new();
            if let Some(max) = constraints.max_length {
                requirements.push(framework_requirement(&ViolationKind::MaxLength(max)));
            }
            if let Some(min) = constraints.min_length {
                requirements.push(framework_requirement(&ViolationKind::MinLength(min)));
            }
            if let Some(pattern) = constraints.pattern {
                requirements.push(pattern_requirement(&composed_pattern(pattern)));
            }
            requirements
        }
        None => vec![match &violation.kind {
            ViolationKind::Pattern(pattern) => pattern_requirement(&composed_pattern(pattern)),
            kind => framework_requirement(kind),
        }],
    }
}

/// The requirement a violation states, for a member reported on its own.
///
/// A `@pattern` is reported UNANCHORED, because the requirement is the expression a
/// value must match and the `^`/`$` are how the model spells "the whole value". Every
/// other constraint keeps the framework's own wording, read off the clause it builds
/// for this same violation, so a constraint kind added to the framework later is
/// reported rather than dropped.
fn requirement(violation: &Violation) -> String {
    match &violation.kind {
        ViolationKind::Pattern(pattern) => pattern_requirement(unanchored(pattern)),
        kind => framework_requirement(kind),
    }
}

/// How a `@pattern` requirement is worded, whatever rendering of the expression it
/// states.
fn pattern_requirement(pattern: &str) -> String {
    format!("Member must satisfy regular expression pattern: {pattern}")
}

/// `pattern` as a composed collection constraint states it: VERBATIM, anchors and all,
/// bracketed by `.*` on each side — the composition is the expression the collection's
/// own rendering has to satisfy, so the element's expression is embedded in it
/// (recorded behaviour).
fn composed_pattern(pattern: &str) -> String {
    format!(".*{pattern}.*")
}

/// `pattern` without its `^` prefix and `$` suffix.
fn unanchored(pattern: &str) -> &str {
    let without_caret = pattern.strip_prefix('^').unwrap_or(pattern);
    without_caret.strip_suffix('$').unwrap_or(without_caret)
}

/// The framework's own wording for `kind`'s requirement.
///
/// Read off a clause the framework builds for a violation of that kind with NO value
/// attached: a clause carrying the value would put caller-controlled text before the
/// marker, so a value containing the marker itself would split inside the value and
/// copy the rest of it into the report.
fn framework_requirement(kind: &ViolationKind) -> String {
    let clause = Violation {
        path: String::new(),
        kind: kind.clone(),
        value: None,
    }
    .message();
    match clause.split_once(CONSTRAINT_MARKER) {
        Some((_, requirement)) => requirement.to_string(),
        None => clause,
    }
}

/// The own constraints of the scalar ELEMENT of a constrained list, as AWS's composed
/// collection-constraint list states them.
#[derive(Debug, Clone, Copy)]
struct ElementConstraints {
    /// The element shape's `@length` maximum, if it declares one.
    max_length: Option<i64>,
    /// The element shape's `@length` minimum, if it declares one.
    min_length: Option<i64>,
    /// The element shape's `@pattern`, verbatim as the model spells it.
    pattern: Option<&'static str>,
}

/// Every list in this service's INPUTS whose element shape carries its own
/// constraints, keyed by the list member's dotted path with the collection steps
/// stripped ([`member_trail`]), so one entry covers that member however deeply it is
/// nested.
///
/// Source: `smithy_query(query="closure",operation="ListDeployments,UntagResource",
/// model_path="services/service-com-amazonaws-launchwizard-launchwizard/smithy-model/\
/// com-amazonaws-launchwizard-launchwizard.json")`. Those two operations hold the only
/// input lists in the model whose element shape is constrained; every other input list
/// (`DeploymentFilterList`) carries a structure element whose own members are reported
/// under their own names.
const LIST_ELEMENT_CONSTRAINTS: &[(&str, ElementConstraints)] = &[
    // `DeploymentFilter$values` -> `DeploymentFilterValues`, element
    // `DeploymentFilterValue`: @length 1..100, @pattern `(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)`.
    (
        "filters.values",
        ElementConstraints {
            max_length: Some(100),
            min_length: Some(1),
            pattern: Some(r"(^(([A-Za-z])+([0-9A-Za-z\-\_])+)$)"),
        },
    ),
    // `UntagResourceInput$tagKeys` -> `TagKeyList`, element `TagKey`: @length 1..128,
    // @pattern `^[a-zA-Z0-9 _.:/=+@-]+$`.
    (
        "tagKeys",
        ElementConstraints {
            max_length: Some(128),
            min_length: Some(1),
            pattern: Some(r"^[a-zA-Z0-9 _.:/=+@-]+$"),
        },
    ),
];

/// The declared element constraints of the list at `collection_path`, if the model
/// gives its elements any.
fn element_constraints(collection_path: &str) -> Option<ElementConstraints> {
    let trail = member_trail(collection_path);
    LIST_ELEMENT_CONSTRAINTS
        .iter()
        .find(|(member, _)| *member == trail)
        .map(|(_, constraints)| *constraints)
}

/// `path` with every collection step removed, leaving the trail of MEMBER names:
/// `filters.1.member.values` -> `filters.values`.
fn member_trail(path: &str) -> String {
    let mut members: Vec<&str> = Vec::new();
    let mut after_index = false;
    for segment in path.split('.') {
        if !segment.is_empty() && segment.chars().all(|c| c.is_ascii_digit()) {
            after_index = true;
            continue;
        }
        let collection_step = after_index && segment == "member";
        after_index = false;
        if !collection_step {
            members.push(segment);
        }
    }
    members.join(".")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLaunchwizardLaunchwizardState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLaunchwizardLaunchwizardState>)
}
