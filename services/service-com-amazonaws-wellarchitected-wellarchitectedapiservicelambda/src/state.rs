// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wellarchitected-wellarchitectedapiservicelambda service.
//!
//! The workload lifecycle (`CreateWorkload`, `GetWorkload`, `ListWorkloads`,
//! `UpdateWorkload`, `DeleteWorkload`) is backed entirely by the maps below, so a
//! create is visible to the next request on the same engine. The record type and its
//! two projections live here rather than in a handler because five operations reach
//! them.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Workload, WorkloadDiscoveryConfig, WorkloadJiraConfigurationInput,
    WorkloadJiraConfigurationOutput, WorkloadSummary, RISK_VALUES,
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

/// How many questions the named lens asks, as this emulator knows it.
///
/// Only the AWS-official Well-Architected Framework lens has a definition here
/// ([`FRAMEWORK_LENS_PILLARS`]); another lens — an AWS-published specialised lens or an
/// imported custom one — carries no question set in this emulator yet, so it adds no
/// questions to a roll-up. `ImportLens`, `GetLens` and `ListLenses` are still stubs,
/// each with its own issue.
pub fn lens_question_count(lens: &str) -> usize {
    if lens_alias(lens) == FRAMEWORK_LENS_ALIAS {
        FRAMEWORK_LENS_PILLARS.iter().map(|(_, count)| count).sum()
    } else {
        0
    }
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

/// Per-service state for com-amazonaws-wellarchitected-wellarchitectedapiservicelambda. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState {
    /// Every stored workload, keyed by its 32-lowercase-hex `WorkloadId`.
    pub workloads: HashMap<String, WorkloadRecord>,
    /// Normalized workload name -> `WorkloadId`. Backs the documented "name must be
    /// unique within an account within a Region; spaces and capitalization are
    /// ignored" rule.
    pub workload_name_index: HashMap<String, String>,
    /// `CreateWorkload`'s `ClientRequestToken` -> what the original request created.
    pub create_workload_tokens: HashMap<String, CreateWorkloadToken>,
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
            self.id_counter = self.id_counter.wrapping_add(1);
            let nanos = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0u128, |d| d.as_nanos());
            let mut hasher = Sha256::new();
            hasher.update(self.id_counter.to_le_bytes());
            hasher.update(nanos.to_le_bytes());
            let id = hex::encode(&hasher.finalize()[..16]);
            if !self.workloads.contains_key(&id) {
                return id;
            }
        }
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
        Some(record)
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

/// The `message` and `Fields` a modelled `ValidationException` carries for one member
/// that failed one constraint. Shared so every operation words a refusal identically;
/// each operation still wraps it in its OWN modelled error enum.
pub fn validation_detail(
    member: &str,
    constraint: &str,
) -> (String, Vec<crate::ops::types::ValidationExceptionField>) {
    (
        format!(
            "1 validation error detected: Value at {member} failed to satisfy constraint: {constraint}"
        ),
        vec![crate::ops::types::ValidationExceptionField {
            name: member.to_string(),
            message: constraint.to_string(),
        }],
    )
}

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
