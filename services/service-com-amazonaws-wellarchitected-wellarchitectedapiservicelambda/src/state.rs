// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wellarchitected-wellarchitectedapiservicelambda service.
//!
//! The workload lifecycle (`CreateWorkload`, `GetWorkload`, `ListWorkloads`,
//! `UpdateWorkload`, `DeleteWorkload`) and the milestones recorded against a workload
//! (`CreateMilestone`, `GetMilestone`) are backed entirely by the maps below, so a
//! create is visible to the next request on the same engine. The custom lenses
//! `ImportLens` imports and `ExportLens` reads back are backed the same way. The record
//! types and their projections live here rather than in a handler because several
//! operations reach them.
//!
//! The modelled `ResourceNotFoundException` / `ConflictException` constructors live
//! here for the same reason: every operation that resolves an identifier or enforces a
//! uniqueness rule must word the failure identically, so there is ONE place each
//! message is built.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    ConflictException, Milestone, ResourceNotFoundException, Workload, WorkloadDiscoveryConfig,
    WorkloadJiraConfigurationInput, WorkloadJiraConfigurationOutput, WorkloadSummary, RISK_VALUES,
};

/// `Workload$ImprovementStatus` before any lens review has been answered. The
/// `WorkloadImprovementStatus` enum's `NOT_APPLICABLE` member — see
/// `smithy_query(query="shape", shape="WorkloadImprovementStatus")`.
pub const DEFAULT_IMPROVEMENT_STATUS: &str = "NOT_APPLICABLE";

/// The `Risk` member every unanswered question of an attached lens counts under —
/// `smithy_query(query="shape", shape="Risk")` lists `UNANSWERED` beside the four
/// answered risks. Named here because the roll-up below fills it from the lens
/// question set rather than from an answer.
pub const RISK_UNANSWERED: &str = "UNANSWERED";

/// `TrustedAdvisorIntegrationStatus$DISABLED`: what a workload's discovery
/// configuration reports while no Trusted Advisor integration has been configured for
/// it. The enum publishes exactly `ENABLED` and `DISABLED`
/// (`smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")`), and a
/// workload nobody enabled the integration for is not integrated.
pub const DEFAULT_TRUSTED_ADVISOR_INTEGRATION_STATUS: &str = "DISABLED";

/// The lens alias of the AWS-official Well-Architected Framework lens — the one lens
/// definition this emulator carries. A lens is named on the wire either by this alias
/// or by its ARN, whose resource part ends in `lens/<alias>`
/// (`smithy_query(query="shape", shape="LensAlias")`).
pub const FRAMEWORK_LENS_ALIAS: &str = "wellarchitected";

/// The AWS Well-Architected Framework lens, as content: its pillars in the order the
/// framework presents them, each with the number of questions that pillar asks.
///
/// `PillarId` is how a pillar is named on the wire (`smithy_query(query="shape",
/// shape="PillarId")`: "The ID used to identify a pillar, for example, `security`").
/// The question counts are the framework's published question numbering —
/// operational excellence OPS 1-11, security SEC 1-11, reliability REL 1-13,
/// performance efficiency PERF 1-5, cost optimization COST 1-11 and sustainability
/// SUS 1-6 — 57 questions in all. Lens content is service-owned data; this emulator
/// ships only this lens's shape, so the questions themselves (which the still-stubbed
/// `GetLens` / `ListAnswers` would return) are not modelled here, only how many each
/// pillar asks.
pub const FRAMEWORK_LENS_PILLARS: &[(&str, usize)] = &[
    ("operationalExcellence", 11),
    ("security", 11),
    ("reliability", 13),
    ("performance", 5),
    ("costOptimization", 11),
    ("sustainability", 6),
];

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// workload.
pub const WORKLOAD_RESOURCE_TYPE: &str = "WORKLOAD";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// milestone.
pub const MILESTONE_RESOURCE_TYPE: &str = "MILESTONE";

/// The ARN service namespace (`OPERATIONS.md`: "ARN namespace: `wellarchitected`").
pub const ARN_NAMESPACE: &str = "wellarchitected";

/// The region an unsigned request is attributed to, when no SigV4 credential scope
/// names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The most workloads this emulator holds for its one account and Region.
///
/// The Well-Architected Tool meters workloads per account per Region and models
/// `ServiceQuotaExceededException` (HTTP 402, "The user has reached their resource
/// quota") on `CreateWorkload` for exactly that case; the real quota is adjustable
/// through Service Quotas, so the number here is this emulator's own ceiling rather
/// than a value read off a doc page. It is what bounds the store's growth: the name
/// index holds one entry per workload, and `remove_workload` drops the create tokens
/// that resolve to a deleted workload, so no token outlives its workload and all three
/// maps are bounded by this one count. It sits far above any realistic client workflow,
/// so a caller never meets it by accident.
pub const MAX_WORKLOADS_PER_ACCOUNT_REGION: usize = 10_000;

/// The most milestones one workload holds: "A workload can have a maximum of 100
/// milestones" (`MilestoneNumber` doc), which is also why `MilestoneNumber` is
/// `@range(min: 1, max: 100)`. Numbers are handed out from 1 upwards and never reused,
/// so this is what bounds the milestone map per workload.
pub const MAX_MILESTONES_PER_WORKLOAD: i32 = 100;

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// lens — the resource type `ShareResourceType` names a lens by
/// (`smithy_query(query="shape", shape="ShareResourceType")` publishes `LENS`).
pub const LENS_RESOURCE_TYPE: &str = "LENS";

/// `ImportLensStatus$IN_PROGRESS`: an import that has been accepted and is still being
/// processed, so the lens it imports is not readable yet
/// (`smithy_query(query="shape", shape="ImportLensStatus")`).
pub const IMPORT_LENS_IN_PROGRESS: &str = "IN_PROGRESS";

/// `ImportLensStatus$COMPLETE`: the import landed and the lens it carried is readable.
pub const IMPORT_LENS_COMPLETE: &str = "COMPLETE";

/// How long after `ImportLens` accepts a lens document the import lands.
///
/// The operation answers with an `ImportLensStatus`, so the import is a job rather than
/// an edit, and AWS publishes no duration for it: this is the emulator's own span, long
/// enough that a caller acting straight after the call still observes `IN_PROGRESS` and
/// the lens not yet readable, short enough not to hold a workflow up.
pub const LENS_IMPORT_DURATION: Duration = Duration::from_millis(500);

/// The `schemaVersion` a custom lens document must declare — the version of the lens
/// JSON format specification the Well-Architected Tool User Guide publishes, which
/// `ImportLens`'s own doc points at ("Lenses are defined in JSON. For more information,
/// see JSON format specification in the Well-Architected Tool User Guide").
pub const LENS_SCHEMA_VERSION: &str = "2021-11-01";

/// The risks a lens document's `riskRules` may assign, from that same format
/// specification. They are the LENS DOCUMENT's own vocabulary, not the API's `Risk`
/// enum: a rule names `HIGH_RISK` where the API reports `HIGH`.
pub const LENS_JSON_RISK_VALUES: &[&str] = &["HIGH_RISK", "MEDIUM_RISK", "NO_RISK"];

/// The condition the LAST risk rule of a question carries: the rule that applies when
/// no earlier rule matched.
pub const LENS_JSON_DEFAULT_CONDITION: &str = "default";

/// The member of `ImportLensInput` a lens-document refusal names.
pub const LENS_JSON_MEMBER: &str = "JSONString";

/// The most custom lenses this emulator holds.
///
/// Custom lenses are metered per account per Region, and `ImportLens` models
/// `ServiceQuotaExceededException` (HTTP 402, "The user has reached their resource
/// quota") for the case; the real quota is adjustable through Service Quotas, so — as
/// with [`MAX_WORKLOADS_PER_ACCOUNT_REGION`] — the number here is this emulator's own
/// ceiling rather than a value read off a doc page. This emulator serves one account
/// ([`::engine::emulator_account_id`]), so the store-wide lens count IS that account's
/// count, and this ceiling is what bounds the lens map's growth. It sits far above any
/// realistic client workflow.
pub const MAX_CUSTOM_LENSES_PER_ACCOUNT_REGION: usize = 1_000;

/// The most `ImportLens` idempotency tokens the store remembers.
///
/// A `ClientRequestToken` is caller-chosen, and an import that UPDATES an existing lens
/// adds no lens, so the lens ceiling above does not bound this ledger: it carries its
/// own, evicting the token remembered longest ago
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::remember_import_token`]),
/// the convention this workspace uses for an idempotency ledger. A caller whose token
/// has been forgotten gets a fresh import rather than a replay, which is also what AWS's
/// own "the result of the original request is returned" window does once it closes.
pub const MAX_REMEMBERED_IMPORT_TOKENS: usize = 10_000;

/// The most tags one resource carries: `TagMap` is `@length(min: 1, max: 50)` wherever a
/// request supplies one (`smithy_query(query="operation", operation="ImportLens")`), so
/// 50 is the most a stored resource can hold too.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One stored workload. Every member of the modelled `Workload` output that a request
/// can set is persisted here, and the members the SERVICE computes — the risk roll-up
/// and the defaulted discovery configuration — are derived from this record by the two
/// `to_*` projections below, the only places a response is built from it.
#[derive(Debug, Clone)]
pub struct WorkloadRecord {
    pub workload_id: String,
    pub workload_arn: String,
    pub workload_name: String,
    pub description: String,
    pub environment: String,
    pub updated_at: SystemTime,
    pub account_ids: Vec<String>,
    pub aws_regions: Vec<String>,
    pub non_aws_regions: Vec<String>,
    pub pillar_priorities: Vec<String>,
    pub architectural_design: Option<String>,
    pub review_owner: Option<String>,
    pub is_review_owner_update_acknowledged: bool,
    pub industry_type: Option<String>,
    pub industry: Option<String>,
    pub notes: Option<String>,
    pub improvement_status: String,
    pub lenses: Vec<String>,
    pub owner: String,
    pub tags: BTreeMap<String, String>,
    pub applications: Vec<String>,
    pub discovery_config: Option<WorkloadDiscoveryConfig>,
    pub jira_configuration: Option<WorkloadJiraConfigurationInput>,
}

impl WorkloadRecord {
    /// This workload's risk roll-up: one entry for every `Risk` the enum publishes, so
    /// a caller that reads a bucket to report review progress always finds it.
    ///
    /// `UNANSWERED` counts the questions of every lens attached to the workload that
    /// carries no answer yet; each other bucket counts the stored answers that carry
    /// that risk. No operation in this emulator can answer a question — `UpdateAnswer`,
    /// `GetAnswer` and `ListAnswers` are still stubs, each with its own issue — so
    /// every question an attached lens asks is unanswered and the answered buckets are
    /// 0. The map is DERIVED from the attached lenses on every read and never stored,
    /// so it moves when the lenses do.
    pub fn risk_counts(&self) -> BTreeMap<String, i32> {
        let mut counts: BTreeMap<String, i32> = RISK_VALUES
            .iter()
            .map(|risk| ((*risk).to_string(), 0))
            .collect();
        // A lens is attached once however many ways the request named it, so resolve
        // each name to its lens and count that lens's questions once.
        let attached: std::collections::BTreeSet<&str> =
            self.lenses.iter().map(|lens| lens_alias(lens)).collect();
        let unanswered: usize = attached.iter().map(|lens| lens_question_count(lens)).sum();
        counts.insert(
            RISK_UNANSWERED.to_string(),
            i32::try_from(unanswered).unwrap_or(i32::MAX),
        );
        counts
    }

    /// This workload's discovery configuration, with the members the service defaults
    /// filled in: a workload always has one, and its Trusted Advisor integration is
    /// [`DEFAULT_TRUSTED_ADVISOR_INTEGRATION_STATUS`] until a caller enables it.
    fn effective_discovery_config(&self) -> WorkloadDiscoveryConfig {
        let stored = self.discovery_config.as_ref();
        WorkloadDiscoveryConfig {
            trusted_advisor_integration_status: stored
                .and_then(|config| config.trusted_advisor_integration_status.clone())
                .or_else(|| Some(DEFAULT_TRUSTED_ADVISOR_INTEGRATION_STATUS.to_string())),
            // The resource definition is what the caller chose to discover resources
            // by; the service defaults nothing for it.
            workload_resource_definition: stored
                .and_then(|config| config.workload_resource_definition.clone()),
        }
    }

    /// The `Workload` shape `GetWorkload`/`UpdateWorkload` return.
    pub fn to_workload(&self) -> Workload {
        Workload {
            workload_id: Some(self.workload_id.clone()),
            workload_arn: Some(self.workload_arn.clone()),
            workload_name: Some(self.workload_name.clone()),
            description: Some(self.description.clone()),
            environment: Some(self.environment.clone()),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            account_ids: Some(self.account_ids.clone()),
            aws_regions: Some(self.aws_regions.clone()),
            non_aws_regions: Some(self.non_aws_regions.clone()),
            architectural_design: self.architectural_design.clone(),
            review_owner: self.review_owner.clone(),
            // Only a workload shared through a share invitation carries these, and
            // workload shares are still a stub.
            review_restriction_date: None,
            share_invitation_id: None,
            is_review_owner_update_acknowledged: Some(self.is_review_owner_update_acknowledged),
            industry_type: self.industry_type.clone(),
            industry: self.industry.clone(),
            notes: self.notes.clone(),
            improvement_status: Some(self.improvement_status.clone()),
            risk_counts: Some(risk_counts_to_map(&self.risk_counts())),
            pillar_priorities: Some(self.pillar_priorities.clone()),
            lenses: Some(self.lenses.clone()),
            owner: Some(self.owner.clone()),
            // The tag map the workload carries, empty when it carries no tags: the
            // `@length(min: 1)` on `TagMap` constrains the tags a REQUEST supplies,
            // not what a projection of the stored workload reports.
            tags: Some(self.tags.clone().into_iter().collect()),
            discovery_config: Some(self.effective_discovery_config()),
            applications: Some(self.applications.clone()),
            // A workload's profiles, and the risk counts restricted to the questions a
            // profile prioritises, exist only once a profile is associated; profiles
            // are still a stub, so there is nothing to project.
            profiles: None,
            prioritized_risk_counts: None,
            jira_configuration: self.jira_configuration.as_ref().map(|input| {
                WorkloadJiraConfigurationOutput {
                    issue_management_status: input.issue_management_status.clone(),
                    issue_management_type: input.issue_management_type.clone(),
                    jira_project_key: input.jira_project_key.clone(),
                    status_message: None,
                }
            }),
        }
    }

    /// The `WorkloadSummary` projection `ListWorkloads` pages over.
    pub fn to_summary(&self) -> WorkloadSummary {
        WorkloadSummary {
            workload_id: Some(self.workload_id.clone()),
            workload_arn: Some(self.workload_arn.clone()),
            workload_name: Some(self.workload_name.clone()),
            owner: Some(self.owner.clone()),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            lenses: Some(self.lenses.clone()),
            risk_counts: Some(risk_counts_to_map(&self.risk_counts())),
            improvement_status: Some(self.improvement_status.clone()),
            profiles: None,
            prioritized_risk_counts: None,
        }
    }
}

fn risk_counts_to_map(counts: &BTreeMap<String, i32>) -> HashMap<String, i32> {
    counts.iter().map(|(k, v)| (k.clone(), *v)).collect()
}

/// One stored milestone: a workload recorded as it stood at one instant.
///
/// The workload is held by VALUE, not by id: "A milestone records the state of a
/// workload at a particular point in time" (`CreateMilestone` doc), so a later
/// `UpdateWorkload` must not change what a recorded milestone reports.
#[derive(Debug, Clone)]
pub struct MilestoneRecord {
    pub milestone_number: i32,
    pub milestone_name: String,
    pub recorded_at: SystemTime,
    pub workload: WorkloadRecord,
}

impl MilestoneRecord {
    /// The `Milestone` shape `GetMilestone` returns.
    pub fn to_milestone(&self) -> Milestone {
        Milestone {
            milestone_number: Some(self.milestone_number),
            milestone_name: Some(self.milestone_name.clone()),
            recorded_at: Some(::aws_smithy_types::DateTime::from(self.recorded_at)),
            workload: Some(self.workload.to_workload()),
        }
    }
}

/// One choice a lens question offers: "choices" in the lens JSON format specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensChoice {
    pub id: String,
    pub title: String,
}

/// One rule that maps the choices selected for a question to the risk the answer
/// carries. `condition` is the format specification's own expression over the
/// question's choice ids, kept VERBATIM: evaluating it is the business of the answer
/// operations (`UpdateAnswer`, `GetAnswer`, `ListAnswers`), each still a stub with its
/// own issue, so this store records the rule rather than interpreting it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensRiskRule {
    pub condition: String,
    pub risk: String,
}

/// One question a lens pillar asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensQuestion {
    pub id: String,
    pub title: String,
    pub choices: Vec<LensChoice>,
    pub risk_rules: Vec<LensRiskRule>,
}

/// One pillar of a lens, with the questions it asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensPillar {
    pub id: String,
    pub name: String,
    pub questions: Vec<LensQuestion>,
}

/// A custom lens as its JSON document defines it, after [`parse_lens_json`] has checked
/// it against the lens JSON format specification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensDefinition {
    pub schema_version: String,
    pub name: String,
    pub description: Option<String>,
    pub pillars: Vec<LensPillar>,
}

impl LensDefinition {
    /// How many questions this lens asks in all — the roll-up denominator a lens review
    /// of this lens counts against.
    pub fn question_count(&self) -> usize {
        self.pillars
            .iter()
            .map(|pillar| pillar.questions.len())
            .sum()
    }

    /// This lens's pillars, in the order the document lists them.
    pub fn pillar_ids(&self) -> Vec<String> {
        self.pillars
            .iter()
            .map(|pillar| pillar.id.clone())
            .collect()
    }
}

/// One version of a lens's content: the document as it was imported, and the definition
/// parsed out of it. The raw document is kept because it is the resource `ExportLens`
/// returns, and the parsed definition because it is what the service reads the lens's
/// name, pillars and questions from.
#[derive(Debug, Clone)]
pub struct LensContent {
    pub json: String,
    pub definition: LensDefinition,
    pub imported_at: SystemTime,
}

/// An import that has been accepted and lands at `completes_at`.
///
/// `ImportLens` answers an `ImportLensStatus`, so the import is a job: until the instant
/// the clock owes it passes, the status is `IN_PROGRESS` and the content is not the
/// lens's content yet. Nothing sleeps for it — [`LensRecord::settled_content`] compares
/// the stored instant with the clock on every read.
#[derive(Debug, Clone)]
pub struct PendingLensImport {
    pub content: LensContent,
    pub completes_at: SystemTime,
}

/// One stored custom lens.
///
/// A lens imported by `ImportLens` "will have a status of DRAFT. The lens cannot be
/// applied to workloads or shared with other Amazon Web Services accounts until it's
/// published with CreateLensVersion" (`ImportLens` doc) — so a record carries its draft
/// content plus the versions that have been published from it, and the latter stays
/// empty until `CreateLensVersion` (its own stub, its own issue) publishes one.
#[derive(Debug, Clone)]
pub struct LensRecord {
    /// The 32-lowercase-hex resource id the lens ARN ends in.
    pub lens_id: String,
    pub lens_arn: String,
    /// The account that imported the lens — its owner.
    pub owner: String,
    pub created_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// The draft content of the last import that LANDED.
    pub draft: Option<LensContent>,
    /// The import still in flight, if one is.
    pub pending: Option<PendingLensImport>,
    /// The published versions of this lens, by `LensVersion`.
    pub versions: BTreeMap<String, LensContent>,
}

impl LensRecord {
    /// The lens's content as of `now`: the pending import's once its instant has
    /// passed, otherwise the last landed draft. `None` while the FIRST import of a lens
    /// is still in flight — there is no lens to read yet.
    pub fn settled_content(&self, now: SystemTime) -> Option<&LensContent> {
        match &self.pending {
            Some(pending) if pending.completes_at <= now => Some(&pending.content),
            _ => self.draft.as_ref(),
        }
    }

    /// The `ImportLensStatus` of this lens's latest import as of `now`.
    pub fn import_status(&self, now: SystemTime) -> &'static str {
        match &self.pending {
            Some(pending) if pending.completes_at > now => IMPORT_LENS_IN_PROGRESS,
            _ => IMPORT_LENS_COMPLETE,
        }
    }

    /// Fold an import whose instant has passed into the draft content. Called before a
    /// WRITE to this record, so a second import never discards a landed one; no read
    /// needs it, because [`Self::settled_content`] already reports the landed content.
    pub fn settle(&mut self, now: SystemTime) {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.completes_at <= now)
        {
            self.draft = self.pending.take().map(|pending| pending.content);
        }
    }
}

/// What an `ImportLens` idempotency token resolves to: the lens the original request
/// imported into, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct ImportLensToken {
    pub lens_id: String,
    pub fingerprint: String,
}

/// Why a lens document was refused.
///
/// The lens JSON format specification the `ImportLens` doc points at is what a document
/// is checked against; this carries the rule it broke, worded from the document itself
/// so the caller can fix it. [`lens_json_exception`] turns it into the modelled
/// `ValidationException`.
#[derive(Debug, Clone)]
pub struct LensJsonRejection {
    /// `ValidationException$Reason` — [`REASON_CANNOT_PARSE`] for a payload that is not
    /// JSON at all, [`REASON_FIELD_VALIDATION_FAILED`] for one that is JSON but not a
    /// lens.
    pub reason: &'static str,
    /// The constraint the document failed, naming where in the document it failed.
    pub constraint: String,
}

/// The modelled `ValidationException` (HTTP 400) for a refused lens document: the
/// service's `CANNOT_PARSE` envelope when the payload is not JSON at all, and its
/// field-validation envelope naming [`LENS_JSON_MEMBER`] otherwise. Built through the
/// same constructors every other refusal in this service uses, so the envelope cannot
/// drift.
pub fn lens_json_exception(
    rejection: &LensJsonRejection,
) -> crate::ops::types::ValidationException {
    let sentence = constraint_sentence(LENS_JSON_MEMBER, &rejection.constraint);
    if rejection.reason == REASON_CANNOT_PARSE {
        cannot_parse_exception(LENS_JSON_MEMBER, &sentence)
    } else {
        field_validation_exception(&[(LENS_JSON_MEMBER.to_string(), sentence)])
    }
}

/// `LensName`'s modelled length bounds (`smithy_query(query="shape", shape="Lens")`:
/// `Name` targets `LensName`, length 1..128), which the lens document's `name` becomes.
const LENS_NAME_LENGTH: (usize, usize) = (1, 128);

/// `LensDescription`'s modelled length bounds (length 1..1024), which the lens
/// document's `description` becomes.
const LENS_DESCRIPTION_LENGTH: (usize, usize) = (1, 1024);

fn lens_json_field(pointer: &str, constraint: &str) -> LensJsonRejection {
    let at = if pointer.is_empty() {
        "the lens document".to_string()
    } else {
        format!("the lens document member {pointer}")
    };
    LensJsonRejection {
        reason: REASON_FIELD_VALIDATION_FAILED,
        constraint: format!("{at} {constraint}"),
    }
}

/// A required string member of a lens document, within `bounds` characters.
fn lens_json_string(
    object: &::serde_json::Value,
    member: &str,
    pointer: &str,
    bounds: (usize, usize),
) -> Result<String, LensJsonRejection> {
    let pointer = format!("{pointer}/{member}");
    let value = object
        .get(member)
        .and_then(::serde_json::Value::as_str)
        .ok_or_else(|| lens_json_field(&pointer, "must be a string"))?;
    let (min, max) = bounds;
    let length = value.chars().count();
    if length < min || length > max {
        return Err(lens_json_field(
            &pointer,
            &format!("must have length between {min} and {max}, inclusive"),
        ));
    }
    Ok(value.to_string())
}

/// A required, non-empty array member of a lens document.
fn lens_json_array<'doc>(
    object: &'doc ::serde_json::Value,
    member: &str,
    pointer: &str,
) -> Result<&'doc Vec<::serde_json::Value>, LensJsonRejection> {
    let pointer = format!("{pointer}/{member}");
    let items = object
        .get(member)
        .and_then(::serde_json::Value::as_array)
        .ok_or_else(|| lens_json_field(&pointer, "must be an array"))?;
    if items.is_empty() {
        return Err(lens_json_field(
            &pointer,
            "must have length greater than or equal to 1",
        ));
    }
    Ok(items)
}

/// Record `id` as taken within `scope`, or refuse it as a duplicate.
fn lens_json_unique(
    seen: &mut std::collections::BTreeSet<String>,
    id: &str,
    pointer: &str,
    scope: &str,
) -> Result<(), LensJsonRejection> {
    if !seen.insert(id.to_string()) {
        return Err(lens_json_field(
            pointer,
            &format!("must be unique within its {scope}"),
        ));
    }
    Ok(())
}

/// The custom lens a JSON document defines, or why it is not a lens.
///
/// The document is checked against the lens JSON format specification the `ImportLens`
/// doc points at ("Lenses are defined in JSON. For more information, see JSON format
/// specification in the Well-Architected Tool User Guide"): a `schemaVersion` of
/// [`LENS_SCHEMA_VERSION`], a `name` and optional `description` within the modelled
/// `LensName` / `LensDescription` bounds, and at least one pillar, each with at least
/// one question, each question with at least one choice and a `riskRules` list whose
/// last rule is the [`LENS_JSON_DEFAULT_CONDITION`] one and whose risks are
/// [`LENS_JSON_RISK_VALUES`]. Ids are unique within their scope, because a rule, an
/// answer and a report all name a question or a choice by its id.
///
/// The `@length(2..500000)` on `LensJSON` ("A custom lens cannot exceed 500 KB in size")
/// is a modelled constraint, so the boundary validator refuses an oversized document
/// before any of this runs.
pub fn parse_lens_json(document: &str) -> Result<LensDefinition, LensJsonRejection> {
    let value: ::serde_json::Value =
        ::serde_json::from_str(document).map_err(|err| LensJsonRejection {
            reason: REASON_CANNOT_PARSE,
            constraint: format!("the lens document is not JSON: {err}"),
        })?;
    if !value.is_object() {
        return Err(lens_json_field("", "must be a JSON object"));
    }

    let schema_version = lens_json_string(&value, "schemaVersion", "", (1, 32))?;
    if schema_version != LENS_SCHEMA_VERSION {
        return Err(lens_json_field(
            "/schemaVersion",
            &format!("must be {LENS_SCHEMA_VERSION}"),
        ));
    }
    let name = lens_json_string(&value, "name", "", LENS_NAME_LENGTH)?;
    let description = match value.get("description") {
        None | Some(::serde_json::Value::Null) => None,
        Some(_) => Some(lens_json_string(
            &value,
            "description",
            "",
            LENS_DESCRIPTION_LENGTH,
        )?),
    };

    let mut pillar_ids = std::collections::BTreeSet::new();
    let mut question_ids = std::collections::BTreeSet::new();
    let mut pillars = Vec::new();
    for (pillar_index, pillar_value) in lens_json_array(&value, "pillars", "")?
        .iter()
        .enumerate()
    {
        let pillar_pointer = format!("/pillars/{pillar_index}");
        let pillar_id = lens_json_string(pillar_value, "id", &pillar_pointer, (1, 128))?;
        lens_json_unique(
            &mut pillar_ids,
            &pillar_id,
            &format!("{pillar_pointer}/id"),
            "lens",
        )?;
        let pillar_name = lens_json_string(pillar_value, "name", &pillar_pointer, LENS_NAME_LENGTH)?;

        let mut questions = Vec::new();
        for (question_index, question_value) in
            lens_json_array(pillar_value, "questions", &pillar_pointer)?
                .iter()
                .enumerate()
        {
            let question_pointer = format!("{pillar_pointer}/questions/{question_index}");
            let question_id = lens_json_string(question_value, "id", &question_pointer, (1, 128))?;
            lens_json_unique(
                &mut question_ids,
                &question_id,
                &format!("{question_pointer}/id"),
                "lens",
            )?;
            let title = lens_json_string(question_value, "title", &question_pointer, (1, 1024))?;

            let mut choice_ids = std::collections::BTreeSet::new();
            let mut choices = Vec::new();
            for (choice_index, choice_value) in
                lens_json_array(question_value, "choices", &question_pointer)?
                    .iter()
                    .enumerate()
            {
                let choice_pointer = format!("{question_pointer}/choices/{choice_index}");
                let choice_id = lens_json_string(choice_value, "id", &choice_pointer, (1, 128))?;
                lens_json_unique(
                    &mut choice_ids,
                    &choice_id,
                    &format!("{choice_pointer}/id"),
                    "question",
                )?;
                choices.push(LensChoice {
                    id: choice_id,
                    title: lens_json_string(choice_value, "title", &choice_pointer, (1, 1024))?,
                });
            }

            let rule_values = lens_json_array(question_value, "riskRules", &question_pointer)?;
            let mut risk_rules = Vec::new();
            for (rule_index, rule_value) in rule_values.iter().enumerate() {
                let rule_pointer = format!("{question_pointer}/riskRules/{rule_index}");
                let condition = lens_json_string(rule_value, "condition", &rule_pointer, (1, 1024))?;
                let risk = lens_json_string(rule_value, "risk", &rule_pointer, (1, 32))?;
                if !LENS_JSON_RISK_VALUES.contains(&risk.as_str()) {
                    return Err(lens_json_field(
                        &format!("{rule_pointer}/risk"),
                        &format!(
                            "must satisfy enum value set: [{}]",
                            LENS_JSON_RISK_VALUES.join(", ")
                        ),
                    ));
                }
                // The last rule is the one that applies when no other matched, so it
                // carries the default condition and no other rule may.
                let is_last = rule_index + 1 == rule_values.len();
                let is_default = condition == LENS_JSON_DEFAULT_CONDITION;
                if is_last != is_default {
                    return Err(lens_json_field(
                        &format!("{rule_pointer}/condition"),
                        &format!(
                            "must be {LENS_JSON_DEFAULT_CONDITION} on the last risk rule of a \
                             question and on no other"
                        ),
                    ));
                }
                risk_rules.push(LensRiskRule { condition, risk });
            }

            questions.push(LensQuestion {
                id: question_id,
                title,
                choices,
                risk_rules,
            });
        }

        pillars.push(LensPillar {
            id: pillar_id,
            name: pillar_name,
            questions,
        });
    }

    Ok(LensDefinition {
        schema_version,
        name,
        description,
        pillars,
    })
}

/// The lens a `LensAlias` names, as its bare alias.
///
/// "For Amazon Web Services official lenses, this is either the lens alias, such as
/// `serverless`, or the lens ARN, such as
/// `arn:aws:wellarchitected:us-east-1::lens/serverless`" (`LensAlias` doc), so an ARN's
/// trailing `lens/<alias>` resolves to the same lens as the bare alias. A lens ARN may
/// also pin a version after the alias, which names the same lens.
pub fn lens_alias(lens: &str) -> &str {
    let alias = lens.rsplit_once("lens/").map_or(lens, |(_, alias)| alias);
    alias.split('/').next().unwrap_or(alias)
}

/// How many questions a lens ATTACHED TO A WORKLOAD asks, as this emulator knows it.
///
/// Only the AWS-official Well-Architected Framework lens has a definition here
/// ([`FRAMEWORK_LENS_PILLARS`]); another lens — an AWS-published specialised lens, or a
/// custom one — carries no question set in this emulator yet, so it adds no questions
/// to a roll-up. An imported custom lens is not a counter-example: `ImportLens` leaves
/// it in DRAFT, and "the lens cannot be applied to workloads or shared with other
/// Amazon Web Services accounts until it's published with CreateLensVersion"
/// (`ImportLens` doc), which is still a stub with its own issue — so no custom lens in
/// this emulator can be attached to a workload for its questions to be counted.
pub fn lens_question_count(lens: &str) -> usize {
    if lens_alias(lens) == FRAMEWORK_LENS_ALIAS {
        FRAMEWORK_LENS_PILLARS.iter().map(|(_, count)| count).sum()
    } else {
        0
    }
}

/// Whether `alias` names `record`: an ARN must agree with the lens's own ARN (so the
/// account and Region it carries have to match), optionally pinning a version after it;
/// a bare id names the lens whose id it is.
fn alias_names_lens(alias: &str, record: &LensRecord) -> bool {
    if !alias.starts_with("arn:") {
        return alias == record.lens_id;
    }
    alias == record.lens_arn
        || alias
            .strip_prefix(record.lens_arn.as_str())
            .is_some_and(|version| version.starts_with('/'))
}

/// Whether a `LensAlias` names an AWS-official lens rather than a custom one.
///
/// "For Amazon Web Services official lenses, this is either the lens alias, such as
/// `serverless`, or the lens ARN, such as `arn:aws:wellarchitected:us-east-1::lens/
/// serverless`. … For custom lenses, this is the lens ARN, such as
/// `arn:aws:wellarchitected:us-west-2:123456789012:lens/0123…`" (`LensAlias` doc): an
/// official lens ARN names NO owner account, because AWS publishes it rather than an
/// account owning it, while a custom lens ARN carries its owner's account id. A bare
/// alias is official too — a custom lens is only ever named by its ARN.
pub fn is_aws_official_lens(alias: &str) -> bool {
    let Some(arn) = alias.strip_prefix("arn:") else {
        return true;
    };
    // partition : service : region : account : resource
    let fields: Vec<&str> = arn.splitn(5, ':').collect();
    fields.len() < 5 || fields[3].is_empty()
}

/// The pillar priority order a workload is created with when the request names none:
/// the framework's own pillar order, which is what orders the items in a review's
/// improvement plan (`WorkloadPillarPriorities` doc: "The priorities of the pillars,
/// which are used to order items in the improvement plan").
pub fn default_pillar_priorities() -> Vec<String> {
    FRAMEWORK_LENS_PILLARS
        .iter()
        .map(|(pillar_id, _)| (*pillar_id).to_string())
        .collect()
}

/// What a `CreateWorkload` idempotency token resolves to: the workload the original
/// request created, plus a fingerprint of that request's parameters so a reuse of the
/// token with DIFFERENT parameters can be told apart from a genuine replay.
#[derive(Debug, Clone)]
pub struct CreateWorkloadToken {
    pub workload_id: String,
    pub fingerprint: String,
}

/// What a `CreateMilestone` idempotency token resolves to: the milestone the original
/// request recorded, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateMilestoneToken {
    pub workload_id: String,
    pub milestone_number: i32,
    pub fingerprint: String,
}

/// Per-service state for com-amazonaws-wellarchitected-wellarchitectedapiservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
    /// Every stored workload, keyed by its 32-lowercase-hex `WorkloadId`.
    pub workloads: HashMap<String, WorkloadRecord>,
    /// Every recorded milestone, keyed by `WorkloadId` then by `MilestoneNumber`. A
    /// `BTreeMap` because the next number is one past the highest already handed out.
    pub milestones: HashMap<String, BTreeMap<i32, MilestoneRecord>>,
    /// `CreateMilestone`'s `ClientRequestToken` -> what the original request recorded.
    pub create_milestone_tokens: HashMap<String, CreateMilestoneToken>,
    /// Normalized workload name -> `WorkloadId`. Backs the documented "name must be
    /// unique within an account within a Region; spaces and capitalization are
    /// ignored" rule.
    pub workload_name_index: HashMap<String, String>,
    /// `CreateWorkload`'s `ClientRequestToken` -> what the original request created.
    pub create_workload_tokens: HashMap<String, CreateWorkloadToken>,
    /// Every stored custom lens, keyed by its 32-lowercase-hex lens id — the resource
    /// part of its ARN, which is how a `LensAlias` names it.
    pub lenses: HashMap<String, LensRecord>,
    /// `ImportLens`'s `ClientRequestToken` -> what the original request imported.
    /// Caller input keys this map, so it is BOUNDED: write it through
    /// [`Self::remember_import_token`], which holds it at
    /// [`MAX_REMEMBERED_IMPORT_TOKENS`].
    pub import_lens_tokens: HashMap<String, ImportLensToken>,
    /// The remembered import tokens in insertion order, so the ledger above evicts the
    /// one remembered longest ago first. A `HashMap` alone cannot say which that is.
    import_lens_token_order: VecDeque<String>,
    /// Monotonic input to [`Self::mint_workload_id`]; never returned to a caller.
    id_counter: u64,
}

impl ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
    /// A fresh `WorkloadId`: 32 lowercase hex characters, as
    /// `^[0-9a-f]{32}$` requires, and unused by any stored workload.
    ///
    /// Derived from a per-store counter and the wall clock — never from the request —
    /// so two identical requests still get distinct ids.
    pub fn mint_workload_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            if !self.workloads.contains_key(&id) {
                return id;
            }
        }
    }

    /// A fresh lens id: 32 lowercase hex characters, the form the `LensAlias` doc's
    /// custom lens ARN ends in (`…:lens/0123456789abcdef01234567890abcdef`), and unused
    /// by any stored lens. Minted the same way, from the store and the clock.
    pub fn mint_lens_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            if !self.lenses.contains_key(&id) {
                return id;
            }
        }
    }

    /// The next 32-lowercase-hex candidate id: a per-store counter and the wall clock,
    /// hashed, so it is never derived from the request.
    fn next_hex_id(&mut self) -> String {
        self.id_counter = self.id_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0u128, |d| d.as_nanos());
        let mut hasher = Sha256::new();
        hasher.update(self.id_counter.to_le_bytes());
        hasher.update(nanos.to_le_bytes());
        hex::encode(&hasher.finalize()[..16])
    }

    /// The stored lens a `LensAlias` names, if one is stored.
    ///
    /// A custom lens is named by its ARN, whose resource part is `lens/<lens id>`, and
    /// [`lens_alias`] is what reduces either spelling to that id — the lens's identity
    /// in this store. An ARN names more than the id, though: a lens belongs to the
    /// account and Region its ARN carries, so an ARN that agrees on the id and disagrees
    /// on either names a DIFFERENT lens and resolves to nothing. A version-pinned ARN
    /// (`…:lens/<id>/<version>`) names the same lens.
    pub fn lens_by_alias(&self, alias: &str) -> Option<&LensRecord> {
        let record = self.lenses.get(lens_alias(alias))?;
        alias_names_lens(alias, record).then_some(record)
    }

    /// [`Self::lens_by_alias`] for a caller that is about to write to the lens.
    pub fn lens_by_alias_mut(&mut self, alias: &str) -> Option<&mut LensRecord> {
        let id = lens_alias(alias).to_string();
        let record = self.lenses.get_mut(&id)?;
        alias_names_lens(alias, record).then_some(record)
    }

    /// Remember what `client_request_token` imported, within
    /// [`MAX_REMEMBERED_IMPORT_TOKENS`].
    ///
    /// Re-recording a token already held replaces its entry and evicts nothing: it names
    /// the same import. A token the ledger has forgotten is not a replay, so the next
    /// request carrying it imports afresh — which is what bounds a stream of fresh
    /// tokens, caller-chosen as they are.
    pub fn remember_import_token(&mut self, client_request_token: String, token: ImportLensToken) {
        if let Some(known) = self.import_lens_tokens.get_mut(&client_request_token) {
            *known = token;
            return;
        }
        while self.import_lens_token_order.len() >= MAX_REMEMBERED_IMPORT_TOKENS {
            match self.import_lens_token_order.pop_front() {
                Some(oldest) => {
                    self.import_lens_tokens.remove(&oldest);
                }
                None => break,
            }
        }
        self.import_lens_tokens
            .insert(client_request_token.clone(), token);
        self.import_lens_token_order.push_back(client_request_token);
    }

    /// The service's stable list order: workload name, then id to break ties between
    /// two names that normalize apart but sort equal.
    pub fn workload_order(&self) -> Vec<(String, String)> {
        let mut keys: Vec<(String, String)> = self
            .workloads
            .values()
            .map(|w| (w.workload_name.clone(), w.workload_id.clone()))
            .collect();
        keys.sort();
        keys
    }

    /// Drop a workload and everything keyed to it: the name that reserved uniqueness
    /// and any create token that resolved to it.
    pub fn remove_workload(&mut self, workload_id: &str) -> Option<WorkloadRecord> {
        let record = self.workloads.remove(workload_id)?;
        self.workload_name_index
            .remove(&normalize_workload_name(&record.workload_name));
        self.create_workload_tokens
            .retain(|_, token| token.workload_id != workload_id);
        // A milestone belongs to its workload, so the whole set goes with it — and with
        // it every create token that resolved to one of them.
        self.milestones.remove(workload_id);
        self.create_milestone_tokens
            .retain(|_, token| token.workload_id != workload_id);
        Some(record)
    }

    /// The workload already holding `name` under the service's own loose comparison
    /// ("spaces and capitalization are ignored"): its name AS STORED and its id, in the
    /// order [`workload_name_conflict`] takes them. The stored name is what a collision
    /// reports, rather than the caller's spelling of it.
    pub fn workload_holding_name(&self, name: &str) -> Option<(&str, &str)> {
        let workload_id = self.workload_name_index.get(&normalize_workload_name(name))?;
        let record = self.workloads.get(workload_id)?;
        Some((
            record.workload_name.as_str(),
            record.workload_id.as_str(),
        ))
    }

    /// The milestone `number` recorded for `workload_id`, if one is.
    pub fn milestone(&self, workload_id: &str, number: i32) -> Option<&MilestoneRecord> {
        self.milestones.get(workload_id)?.get(&number)
    }

    /// The number the next milestone of `workload_id` takes: one past the highest ever
    /// handed out, starting at 1. Can exceed [`MAX_MILESTONES_PER_WORKLOAD`], which is
    /// how a caller meets the quota.
    pub fn next_milestone_number(&self, workload_id: &str) -> i32 {
        self.milestones
            .get(workload_id)
            .and_then(|recorded| recorded.keys().next_back().copied())
            .unwrap_or(0)
            .saturating_add(1)
    }
}

/// The form a workload name is compared in: "Spaces and capitalization are ignored
/// when checking for uniqueness" (`CreateWorkloadInput$WorkloadName` doc).
pub fn normalize_workload_name(name: &str) -> String {
    name.chars()
        .filter(|c| !c.is_whitespace())
        .flat_map(char::to_lowercase)
        .collect()
}

/// A stable fingerprint of an idempotent request's parameters.
pub fn parameter_fingerprint(parameters: &::serde_json::Value) -> String {
    let mut hasher = Sha256::new();
    hasher.update(parameters.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

/// Encode a `ListWorkloads` cursor as an opaque, self-describing `NextToken`.
///
/// Hex of the JSON cursor: it satisfies the modelled `NextToken` pattern
/// `^[A-Za-z0-9+/=_-]+$` and carries the whole cursor, so paging needs no
/// server-side token table — a reporting operation must not write to state.
pub fn encode_next_token(workload_name: &str, workload_id: &str) -> String {
    let cursor = ::serde_json::json!({ "n": workload_name, "i": workload_id });
    hex::encode(cursor.to_string())
}

/// The `(workload name, workload id)` a `NextToken` points just past, or `None` when
/// the token is not one this service issued.
pub fn decode_next_token(token: &str) -> Option<(String, String)> {
    let bytes = hex::decode(token).ok()?;
    let cursor: ::serde_json::Value = ::serde_json::from_slice(&bytes).ok()?;
    let name = cursor.get("n")?.as_str()?.to_string();
    let id = cursor.get("i")?.as_str()?.to_string();
    Some((name, id))
}

/// `ValidationExceptionReason$FIELD_VALIDATION_FAILED` — the modelled classification for
/// a request whose members were read but whose values the service refused. See
/// `VALIDATION_EXCEPTION_REASON_VALUES` for the enum's whole value set.
pub const REASON_FIELD_VALIDATION_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// `ValidationExceptionReason$CANNOT_PARSE` — the modelled classification for a value
/// the service could not decode at all (an opaque pagination token, say), as opposed to
/// one that was decoded and then failed a field rule.
pub const REASON_CANNOT_PARSE: &str = "CANNOT_PARSE";

/// The modelled `ValidationException` (HTTP 400) for members whose values the service
/// refused: `Reason` is `FIELD_VALIDATION_FAILED`, `Fields` carries one entry per
/// offending member, and the required `Message` states each distinct field message once
/// (a cross-member rule names every member it covers with the same sentence, and
/// `Message` must not repeat it per member).
///
/// `Message` carries this service's `[Validation]` tag, the same form
/// [`tagged_message`] gives every other exception class; a `Fields` entry does NOT —
/// the tag names the exception's class, and a field finding is a clause inside it.
///
/// Every operation builds this refusal class here, so the envelope cannot drift between
/// operations; each operation still wraps the result in its OWN modelled error enum.
pub fn field_validation_exception(
    fields: &[(String, String)],
) -> crate::ops::types::ValidationException {
    let mut distinct: Vec<&str> = Vec::new();
    for (_, message) in fields {
        if !distinct.contains(&message.as_str()) {
            distinct.push(message);
        }
    }
    crate::ops::types::ValidationException {
        message: tagged_message(VALIDATION_TAG, &distinct.join(" ")),
        reason: Some(REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(
            fields
                .iter()
                .map(|(name, message)| crate::ops::types::ValidationExceptionField {
                    name: name.clone(),
                    message: message.clone(),
                })
                .collect(),
        ),
    }
}

/// The modelled `ValidationException` for a value the service could not decode:
/// `Reason` `CANNOT_PARSE`, which tells the caller the value cannot be corrected, only
/// obtained again, plus one `Fields` entry naming the member whose value could not be
/// decoded.
pub fn cannot_parse_exception(
    member: &str,
    message: &str,
) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: tagged_message(VALIDATION_TAG, message),
        reason: Some(REASON_CANNOT_PARSE.to_string()),
        fields: Some(vec![crate::ops::types::ValidationExceptionField {
            name: member.to_string(),
            message: message.to_string(),
        }]),
    }
}

/// The modelled `ValidationException` for a request the service refused without
/// classifying it: only the `@required` `Message`. `ValidationExceptionReason` has no
/// value for "a member the request had to carry is absent" (`UNKNOWN_OPERATION`,
/// `CANNOT_PARSE`, `FIELD_VALIDATION_FAILED`, `OTHER`), and the recorded service leaves
/// `Reason` and `Fields` off that answer rather than inventing one.
pub fn unclassified_exception(message: String) -> crate::ops::types::ValidationException {
    crate::ops::types::ValidationException {
        message: tagged_message(VALIDATION_TAG, &message),
        reason: None,
        fields: None,
    }
}

/// The failure class every `ValidationException` message this service words opens with
/// (see [`tagged_message`]).
const VALIDATION_TAG: &str = "Validation";

/// The sentence a member-constraint failure states, in the form the framework's own
/// constraint walk uses so a handler-enforced rule reads like a modelled one.
pub fn constraint_sentence(member: &str, constraint: &str) -> String {
    format!("Value at {member} failed to satisfy constraint: {constraint}")
}

/// The refusal for one member that failed one constraint: the sentence above, reported
/// as a field validation naming that member.
pub fn member_constraint_exception(
    member: &str,
    constraint: &str,
) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        member.to_string(),
        constraint_sentence(member, constraint),
    )])
}

/// The tag this service opens an exception message with: the failure class in square
/// brackets, then the sentence — `[ResourceNotFound] …`, `[Conflict] …`,
/// `[Validation] …`. Every failure this service words goes through here, so one form
/// covers them all.
fn tagged_message(tag: &str, sentence: &str) -> String {
    format!("[{tag}] {sentence}")
}

/// A modelled `ResourceNotFoundException` (HTTP 404) for a lookup that resolved
/// nothing.
///
/// The message is built from the request and the owning account: it names the resource
/// that was looked for (`subject`, e.g. `workload with ID <id>`) and the account it was
/// looked for in, so a caller can tell "not in this account / not shared with me" from
/// "deleted". `resource_id` is the identifier AS SENT, and `resource_type` the modelled
/// type of the resource that was missing — both are `@required` on the shape.
pub fn resource_not_found(
    resource_type: &str,
    resource_id: String,
    subject: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: tagged_message(
            "ResourceNotFound",
            &format!(
                "No {subject} was found in account {}.",
                ::engine::emulator_account_id()
            ),
        ),
        resource_id,
        resource_type: resource_type.to_string(),
    }
}

/// [`resource_not_found`] for a workload id that names no stored workload. Shared by
/// every operation that resolves a `WorkloadId`, so they cannot word it differently.
pub fn workload_not_found(workload_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        WORKLOAD_RESOURCE_TYPE,
        workload_id.to_string(),
        &format!("workload with ID {workload_id}"),
    )
}

/// [`resource_not_found`] for a milestone number that names no recorded milestone of
/// `workload_id`.
///
/// A milestone is identified by the pair, so the `ResourceId` is the composite
/// `<workloadId>.<milestoneNumber>` and the message names both.
pub fn milestone_not_found(workload_id: &str, milestone_number: i32) -> ResourceNotFoundException {
    resource_not_found(
        MILESTONE_RESOURCE_TYPE,
        milestone_resource_id(workload_id, milestone_number),
        &format!(
            "milestone with number {milestone_number} in a workload with ID {workload_id}"
        ),
    )
}

/// How a milestone is named in an exception's `ResourceId`: the workload it belongs to
/// and its number, which together identify it.
pub fn milestone_resource_id(workload_id: &str, milestone_number: i32) -> String {
    format!("{workload_id}.{milestone_number}")
}

/// [`resource_not_found`] for a `LensAlias` that names no readable custom lens — none
/// is stored under it, or the import that would produce it has not landed yet.
///
/// The `ResourceId` is the alias AS SENT, which is how the request named the lens.
pub fn lens_not_found(alias: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("custom lens with alias {alias}"),
    )
}

/// [`resource_not_found`] for a `LensVersion` of a stored lens that has not been
/// published. A version exists only once `CreateLensVersion` publishes it.
pub fn lens_version_not_found(alias: &str, version: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("version {version} of the custom lens with alias {alias}"),
    )
}

/// A modelled `ConflictException` (HTTP 409) for a request that collides with a
/// resource that already exists.
///
/// The message names the resource that is in the way AS STORED, and `resource_id`
/// identifies it, so a caller can fetch it. It is deliberately the stored value rather
/// than the spelling the request submitted: the two differ wherever the uniqueness rule
/// compares loosely (workload names ignore spaces and capitalization), and what the
/// caller needs named is the resource it collided with.
pub fn conflict(resource_type: &str, resource_id: String, sentence: &str) -> ConflictException {
    ConflictException {
        message: tagged_message("Conflict", sentence),
        resource_id,
        resource_type: resource_type.to_string(),
    }
}

/// [`conflict`] for a workload name that is already taken: "The name must be unique
/// within an account within an Amazon Web Services Region. Spaces and capitalization
/// are ignored when checking for uniqueness" (`CreateWorkloadInput$WorkloadName` doc).
///
/// `existing_name` / `existing_id` are the stored name and id of the workload already
/// holding the name, not the request's spelling of it.
pub fn workload_name_conflict(existing_name: &str, existing_id: &str) -> ConflictException {
    conflict(
        WORKLOAD_RESOURCE_TYPE,
        existing_id.to_string(),
        &format!("A workload with name {existing_name} already exists."),
    )
}

/// [`conflict`] for a milestone name that is already taken within its workload:
/// "Milestone names must be unique within a workload"
/// (`CreateMilestoneInput$MilestoneName` doc). `existing_name` / `existing_number` are
/// the stored name and number of the milestone already holding it.
pub fn milestone_name_conflict(
    existing_name: &str,
    workload_id: &str,
    existing_number: i32,
) -> ConflictException {
    conflict(
        MILESTONE_RESOURCE_TYPE,
        milestone_resource_id(workload_id, existing_number),
        &format!("A milestone with name {existing_name} already exists."),
    )
}

/// [`conflict`] for an idempotency token replayed with different parameters: the token
/// already names a resource, so the second request cannot be the same request.
pub fn idempotency_token_conflict(
    resource_type: &str,
    resource_id: String,
    client_request_token: &str,
) -> ConflictException {
    conflict(
        resource_type,
        resource_id,
        &format!(
            "A request with ClientRequestToken {client_request_token} was already \
             submitted with different parameters."
        ),
    )
}

/// The refusal for a member the request had to carry and did not: the required
/// `Message`, worded exactly as the framework words an absent modelled member, so a
/// member this service requires by documentation reads like one the model requires.
/// No `Reason` and no `Fields` — see [`unclassified_exception`].
pub fn absent_member_exception(member: &str) -> crate::ops::types::ValidationException {
    unclassified_exception(::aws_smithy_schema_server::missing_member_message(member))
}

/// The refusal for a workload that names neither `AwsRegions` nor `NonAwsRegions`:
/// "Either AwsRegions, NonAwsRegions, or both must be specified when creating a
/// workload" (the `CreateWorkload` doc). The rule spans both members, so `Fields`
/// carries an entry for EACH of them with the one sentence that states it, and
/// `Message` states that sentence once.
pub fn either_regions_exception() -> crate::ops::types::ValidationException {
    let sentence = format!("Either {AWS_REGIONS_MEMBER} or {NON_AWS_REGIONS_MEMBER} is required.");
    field_validation_exception(&[
        (AWS_REGIONS_MEMBER.to_string(), sentence.clone()),
        (NON_AWS_REGIONS_MEMBER.to_string(), sentence),
    ])
}

/// Every Amazon Web Services Region id a workload may name in `AwsRegions`.
///
/// This model types `AwsRegion` as a plain string ("An Amazon Web Services Region, for
/// example, `us-west-2` or `ap-northeast-1`" — `smithy_query(query="shape",
/// shape="AwsRegion")`), so the value set is not in the model: it is the published
/// Region list from the AWS General Reference ("Regions, Availability Zones, and Local
/// Zones"), which other AWS Smithy models enumerate verbatim as an `AwsRegion` enum.
/// Sorted, so a reader can see at a glance what is and is not here.
pub const AWS_REGIONS: &[&str] = &[
    "af-south-1",
    "ap-east-1",
    "ap-east-2",
    "ap-northeast-1",
    "ap-northeast-2",
    "ap-northeast-3",
    "ap-south-1",
    "ap-south-2",
    "ap-southeast-1",
    "ap-southeast-2",
    "ap-southeast-3",
    "ap-southeast-4",
    "ap-southeast-5",
    "ap-southeast-6",
    "ap-southeast-7",
    "ca-central-1",
    "ca-west-1",
    "cn-north-1",
    "cn-northwest-1",
    "eu-central-1",
    "eu-central-2",
    "eu-isoe-west-1",
    "eu-north-1",
    "eu-south-1",
    "eu-south-2",
    "eu-west-1",
    "eu-west-2",
    "eu-west-3",
    "eusc-de-east-1",
    "il-central-1",
    "me-central-1",
    "me-south-1",
    "mx-central-1",
    "sa-east-1",
    "us-east-1",
    "us-east-2",
    "us-gov-east-1",
    "us-gov-west-1",
    "us-iso-east-1",
    "us-iso-west-1",
    "us-isob-east-1",
    "us-isob-west-1",
    "us-isof-east-1",
    "us-isof-south-1",
    "us-west-1",
    "us-west-2",
];

/// The refusal for an `AwsRegions` entry that is not a Region, or `None` when every
/// entry is one. A workload records the Regions it runs in, so a value no Region
/// carries can never be true of the resource and is refused before any state is
/// written; `NonAwsRegions` is deliberately NOT checked, because it names Regions
/// outside AWS, which no published list bounds.
pub fn unknown_aws_region_exception(
    regions: &[String],
) -> Option<crate::ops::types::ValidationException> {
    let unknown: Vec<(String, String)> = regions
        .iter()
        .filter(|region| !AWS_REGIONS.contains(&region.as_str()))
        .map(|region| {
            (
                AWS_REGIONS_MEMBER.to_string(),
                format!("Unknown region {region}."),
            )
        })
        .collect();
    (!unknown.is_empty()).then(|| field_validation_exception(&unknown))
}

/// The modelled member name an unknown-Region finding reports
/// (`CreateWorkloadInput$AwsRegions` / `UpdateWorkloadInput$AwsRegions`).
const AWS_REGIONS_MEMBER: &str = "AwsRegions";

/// The modelled member that satisfies the cross-member Regions rule in place of
/// `AwsRegions` (`CreateWorkloadInput$NonAwsRegions`).
const NON_AWS_REGIONS_MEMBER: &str = "NonAwsRegions";

/// The region this request was signed for, from its SigV4 credential scope; an
/// unsigned request is attributed to [`DEFAULT_REGION`].
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState>)
}
