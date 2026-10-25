// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wellarchitected-wellarchitectedapiservicelambda service.
//!
//! The workload lifecycle (`CreateWorkload`, `GetWorkload`, `ListWorkloads`,
//! `UpdateWorkload`, `DeleteWorkload`), the milestones recorded against a workload
//! (`CreateMilestone`, `GetMilestone`, `ListMilestones`), the review templates
//! (`CreateReviewTemplate`, `GetReviewTemplate`, `ListReviewTemplates`,
//! `DeleteReviewTemplate`), the lens reviews of a workload (`GetLensReview`,
//! `ListLensReviews`, `UpdateLensReview`), the answers of
//! a workload's lens reviews (`GetAnswer`, `ListAnswers`, `UpdateAnswer`), the
//! improvement plan of one of those lens
//! reviews (`ListLensReviewImprovements`) and the answers of a review template's lens
//! reviews (`ListReviewTemplateAnswers`, `GetReviewTemplateAnswer`,
//! `UpdateReviewTemplateAnswer`) are backed entirely by the maps below, so a
//! create is visible to the next request on the same engine. A template's lens review is
//! held against the lens VERSION it was applied at, which
//! `UpgradeReviewTemplateLensReview` moves to the lens's latest
//! ([`ReviewTemplateLensReviewRecord::lens_version`]). The custom lenses
//! `ImportLens`
//! imports, the versions `CreateLensVersion` publishes from them, the content
//! `ExportLens` reads back and the versions `GetLensVersionDifference` compares are backed
//! the same way, and `ListLenses` pages over those
//! lenses beside the AWS-official lens this emulator ships. The Well-Architected
//! Tool profiles (`CreateProfile`, `GetProfile`) are backed the same way, each keyed by the
//! `ProfileArn` the create minted, and the association of one with a workload
//! (`AssociateProfiles`, `DisassociateProfiles`) is held on the workload the association is
//! OF, pinned at the profile version the workload is reviewed against — the version
//! `UpgradeProfileVersion` moves to the one that profile is at. The agent profiles
//! (`CreateAgentProfile`, `GetAgentProfile`, `ListAgentProfiles`) and the optimization
//! goals recorded against one (`CreateAgentGoal`, `GetAgentGoal`, `UpdateAgentGoal`,
//! `DeleteAgentGoal`) are backed the same way,
//! each goal keyed under the profile it belongs to. The contexts recorded against a
//! profile (`CreateAgentContext`, `GetAgentContext`, `ListAgentContexts`) are held
//! exactly like the goals, keyed under the profile they belong to. The recommendation
//! generations of a profile (`StartAgentRecommendationGeneration`,
//! `GetAgentRecommendationGeneration`) are held the same way; a generation is a JOB, so
//! its status, progress and transition timestamps are DERIVED from the instants the
//! clock owes its transitions at ([`AgentGenerationRecord`]) rather than stored, and no
//! read has to write. The record types and
//! their projections live here rather than in a handler because several operations reach
//! them.
//!
//! The lens content this emulator ships itself — the AWS-official lens catalog, whose
//! entries carry each official lens's name, version, pillars and question set
//! ([`official_lenses`]) — lives here too, for the same reason: the risk roll-up of a
//! workload and the answer list of a lens review must agree about which questions a lens
//! asks. The aliases of the lenses Amazon Web Services publishes
//! ([`AWS_OFFICIAL_LENS_ALIASES`]) live beside it, because a lens is applied BY NAME and
//! every operation that applies one has to resolve that name against the same catalog
//! ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::resolve_lens`]) —
//! the aliases are every lens AWS publishes, of which the content catalog carries the ones
//! this emulator holds questions for.
//!
//! The modelled `ResourceNotFoundException` / `ConflictException` constructors live
//! here for the same reason: every operation that resolves an identifier or enforces a
//! uniqueness rule must word the failure identically, so there is ONE place each
//! message is built.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

pub mod official_lenses;

pub use official_lenses::{
    framework_lens_published_at, lens_metadata, lens_pillars, lens_question, lens_question_count,
    lens_questions, official_lens, LensMetadata, OfficialLens, OFFICIAL_LENS_CATALOG,
    SPECIALISED_LENS_VERSION, WELL_ARCHITECTED_PILLARS,
};

use crate::ops::types::{
    AgentProfileSummary, AggregationConfiguration, Choice, ChoiceAnswer, ConflictException,
    ContextContent,
    ContextSummary,
    GoalSummary, JiraSelectedQuestionConfiguration, LensReview, LensSummary, Milestone,
    MilestoneSummary, PillarReviewSummary,
    Profile, ProfileQuestion, Progress,
    ResourceNotFoundException,
    ReviewTemplate, ReviewTemplateAnswer, ReviewTemplateLensReview,
    ReviewTemplatePillarReviewSummary,
    ReviewTemplateSummary, Scope,
    ShareInvitation, ShareInvitationSummary, Tag, Workload, WorkloadDiscoveryConfig,
    WorkloadJiraConfigurationInput, WorkloadJiraConfigurationOutput, WorkloadShare,
    WorkloadShareSummary, WorkloadSummary, APPLICATION_TYPE_VALUES, CONTEXT_TYPE_VALUES,
    CRITICALITY_VALUES, QUESTION_VALUES, RISK_VALUES,
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

/// The four `Risk` members an ANSWERED question can carry
/// (`smithy_query(query="shape", shape="Risk")`), named here because
/// [`AnswerRecord::risk`] derives one of them from every stored answer.
pub const RISK_HIGH: &str = "HIGH";
/// See [`RISK_HIGH`].
pub const RISK_MEDIUM: &str = "MEDIUM";
/// See [`RISK_HIGH`].
pub const RISK_NONE: &str = "NONE";
/// See [`RISK_HIGH`].
pub const RISK_NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// The `ChoiceStatus` member that says a best practice is NOT in place
/// (`smithy_query(query="shape", shape="ChoiceStatus")`): what keeps a question's risk
/// off `NONE`.
pub const CHOICE_STATUS_UNSELECTED: &str = "UNSELECTED";

/// The `ChoiceStatus` member that says a best practice IS in place
/// (`smithy_query(query="shape", shape="ChoiceStatus")`): the status every `ChoiceId` a
/// `SelectedChoices` names carries, and the only status a selected choice carries — which
/// is what keeps an answer's `SelectedChoices` and its `ChoiceAnswers` in agreement
/// ([`AnswerRecord::resolve_selected_choices`]).
pub const CHOICE_STATUS_SELECTED: &str = "SELECTED";

/// `TrustedAdvisorIntegrationStatus$DISABLED`: what a workload's discovery
/// configuration reports while no Trusted Advisor integration has been configured for
/// it. The enum publishes exactly `ENABLED` and `DISABLED`
/// (`smithy_query(query="shape", shape="TrustedAdvisorIntegrationStatus")`), and a
/// workload nobody enabled the integration for is not integrated.
pub const DEFAULT_TRUSTED_ADVISOR_INTEGRATION_STATUS: &str = "DISABLED";

/// The lens alias of the AWS-official Well-Architected Framework lens — the first entry
/// of the AWS-official lens catalog this emulator publishes
/// ([`official_lenses::OFFICIAL_LENS_CATALOG`]). A lens is named on the wire either by its
/// alias or by its ARN, whose resource part ends in `lens/<alias>`
/// (`smithy_query(query="shape", shape="LensAlias")`).
pub const FRAMEWORK_LENS_ALIAS: &str = "wellarchitected";

/// The `LensSummary$Description` of the AWS-official Well-Architected Framework lens —
/// what that lens's question set ([`FRAMEWORK_LENS_QUESTIONS`]) asks
/// about, within `LensDescription`'s modelled 1..1024 characters
/// (`smithy_query(query="shape", shape="LensSummary")`). Catalog content rather than a
/// stored value, so it lives beside the question set it describes, next to that lens's
/// name and version ([`FRAMEWORK_LENS_NAME`], [`FRAMEWORK_LENS_VERSION`]).
pub const FRAMEWORK_LENS_DESCRIPTION: &str = "The Amazon Web Services Well-Architected \
     Framework provides a consistent approach for customers and partners to evaluate \
     architectures, and implement designs that scale over time.";

/// `LensSummary$LensStatus` for a lens whose current content no version has been
/// published from.
///
/// "The new or updated lens will have a status of DRAFT. The lens cannot be applied to
/// workloads or shared with other Amazon Web Services accounts until it's published with
/// CreateLensVersion" (`ImportLens` doc) — so this is the status of a lens that has only
/// been imported, and of one imported OVER after a version was published from it.
///
/// It is NOT a member of the modelled `LensStatus` enum, which publishes `CURRENT`,
/// `NOT_CURRENT`, `DEPRECATED`, `DELETED` and `UNSHARED` (`smithy_query(query="shape",
/// shape="LensStatus")`) — the version-currency vocabulary a workload's lens REVIEW
/// reports, which is what [`LENS_STATUS_CURRENT`] is for. What a lens SUMMARY reports
/// there is the lens's publication state, the vocabulary its own filter is defined over:
/// `LensStatusType` is exactly `ALL`, `DRAFT` and `PUBLISHED`
/// (`smithy_query(query="shape", shape="LensStatusType")`), so `ListLenses` filters on
/// the member it is named after.
pub const LENS_STATUS_DRAFT: &str = "DRAFT";

/// `LensSummary$LensVersion` for a lens whose current content no version has been
/// published from.
///
/// A custom lens's versions are named by the caller (`CreateLensVersion`'s `LensVersion`
/// is an input), so an unpublished lens has no version NAME of its own; what it has is its
/// draft, and that is what the service reports as the version of the content a caller
/// would read — the same word `LensSummary$LensStatus` reports ([`LENS_STATUS_DRAFT`]),
/// because both members speak about that one content.
pub const LENS_VERSION_DRAFT: &str = "DRAFT";

/// `LensSummary$LensStatus` for a lens a version HAS been published from — including the
/// lenses Amazon Web Services publishes itself, whose content is the version they are
/// published at. See [`LENS_STATUS_DRAFT`] for why these two and not the modelled
/// `LensStatus` members.
pub const LENS_STATUS_PUBLISHED: &str = "PUBLISHED";

/// `LensType$AWS_OFFICIAL`: a lens Amazon Web Services publishes
/// (`smithy_query(query="shape", shape="LensType")`).
pub const LENS_TYPE_AWS_OFFICIAL: &str = "AWS_OFFICIAL";

/// `LensType$CUSTOM_SELF`: a custom lens the CALLING account owns.
pub const LENS_TYPE_CUSTOM_SELF: &str = "CUSTOM_SELF";

/// `LensType$CUSTOM_SHARED`: a custom lens another account owns and shared with the
/// caller.
pub const LENS_TYPE_CUSTOM_SHARED: &str = "CUSTOM_SHARED";

/// `LensStatusType$ALL`: every lens, whatever its publication state
/// (`smithy_query(query="shape", shape="LensStatusType")`, which publishes `ALL`,
/// `DRAFT` and `PUBLISHED`).
///
/// This is also what a `ListLenses` request that names NO `LensStatus` selects: the
/// member filters the list, and a request that asks for no filtering gets none. An
/// unpublished lens is still a lens the account has — `ImportLens` answers its ARN, and
/// `ListLenses` is how a caller finds it again — so it is listed too, with the publication
/// state of each reported lens being what `DRAFT` and `PUBLISHED` then narrow to.
pub const LENS_STATUS_TYPE_ALL: &str = "ALL";

/// `LensStatusType$DRAFT`: only lenses carrying content no version has been published
/// from.
pub const LENS_STATUS_TYPE_DRAFT: &str = "DRAFT";

/// `LensStatusType$PUBLISHED`: only lenses a published version exists of — the lenses
/// that can actually be applied to a workload, since a lens that has never been published
/// "cannot be applied to workloads or shared with other Amazon Web Services accounts"
/// (`ImportLens` doc).
pub const LENS_STATUS_TYPE_PUBLISHED: &str = "PUBLISHED";

/// The `LensName` of the AWS-official Well-Architected Framework lens: the name AWS
/// publishes for the lens [`FRAMEWORK_LENS_ALIAS`] names.
pub const FRAMEWORK_LENS_NAME: &str = "Well-Architected Framework";

/// The `LensVersion` the framework lens content in this file is: the revision of the
/// Well-Architected Framework lens whose published question numbering
/// [`FRAMEWORK_LENS_QUESTIONS`] carries (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5,
/// COST 1-11, SUS 1-6). It names THIS catalog's version, so a change to the catalog
/// changes it; `LensVersion` is `@length(min: 1, max: 32)`
/// (`smithy_query(query="shape", shape="LensReview")`).
pub const FRAMEWORK_LENS_VERSION: &str = "2023-10-03";

/// `LensStatus$CURRENT`: the lens review is against the lens's latest version
/// (`smithy_query(query="shape", shape="LensStatus")` publishes `CURRENT`,
/// `NOT_CURRENT`, `DEPRECATED`, `DELETED` and `UNSHARED`). Each AWS-official lens in the
/// catalog carries exactly one version (its [`OfficialLens::version`]), so a review of one
/// is on that lens's current version; a custom lens's versions are what
/// `CreateLensVersion` publishes, and a review stays on the version it was applied at
/// until it is upgraded.
pub const LENS_STATUS_CURRENT: &str = "CURRENT";

/// `LensStatus$NOT_CURRENT`: the lens review is against a version of the lens a later one
/// has superseded — what a review reports between the publish that supersedes its version
/// and the upgrade that moves it to the latest one.
pub const LENS_STATUS_NOT_CURRENT: &str = "NOT_CURRENT";

/// One question of the AWS-official framework lens, as this emulator's lens content:
/// the pillar it belongs to, the id it is named by on the wire, its title and the best
/// practices it asks about.
///
/// This is the STATIC catalog's entry, distinct from [`LensQuestion`] — the question of
/// a custom lens DOCUMENT, which carries the choice and risk-rule content that document
/// supplies.
#[derive(Debug, Clone, Copy)]
pub struct FrameworkQuestion {
    /// `AnswerSummary$PillarId` — the pillar this question belongs to, one of
    /// [`WELL_ARCHITECTED_PILLARS`].
    pub pillar_id: &'static str,
    /// `AnswerSummary$QuestionId` (`@length(1, 128)`), stable for the life of the lens.
    pub question_id: &'static str,
    /// `AnswerSummary$QuestionTitle` (`@length(1, 512)`).
    pub title: &'static str,
    /// The best practices this question asks about, in the lens's own order: the
    /// `Choice$Title` of each choice the question offers. Each is the Well-Architected
    /// Framework's published best practice for this question, and its position in this
    /// list is what [`FrameworkQuestion::choice_id`] names it by.
    pub best_practices: &'static [&'static str],
}

impl FrameworkQuestion {
    /// The `ChoiceId` of the best practice at `index` in [`Self::best_practices`]:
    /// `<question id>-bp-<two-digit position>`.
    ///
    /// A lens's wire CHOICE ids are lens content, which neither the Smithy model nor the
    /// service docs publish — the same reason the `question_id`s in
    /// [`FRAMEWORK_LENS_QUESTIONS`] are this emulator's own — so they are derived from
    /// the question that offers the choice and the choice's position in that question's
    /// catalog. That makes them unique within the question, stable for the life of the
    /// catalog (an answer stored under one keeps resolving), and inside the modelled
    /// `ChoiceId` `@length(min: 1, max: 64)`.
    pub fn choice_id(&self, index: usize) -> String {
        format!("{}-bp-{:02}", self.question_id, index.saturating_add(1))
    }

    /// This catalog row as the question a LENS REVIEW asks
    /// ([`ReviewQuestion`]): the one projection of a static catalog entry into the
    /// question set every answer operation reads and pages over, so an official lens's
    /// questions and a custom lens's are reported through the same shape.
    ///
    /// Each best practice becomes a choice whose `ChoiceId` is derived from the question
    /// and the practice's position ([`Self::choice_id`]), because an official lens's wire
    /// choice ids are this emulator's own; a custom lens's choices carry the ids ITS
    /// document names them by.
    pub fn to_review_question(&self) -> ReviewQuestion {
        ReviewQuestion {
            pillar_id: self.pillar_id.to_string(),
            question_id: self.question_id.to_string(),
            title: self.title.to_string(),
            best_practices: self
                .best_practices
                .iter()
                .enumerate()
                .map(|(index, best_practice)| LensChoice {
                    id: self.choice_id(index),
                    title: (*best_practice).to_string(),
                })
                .collect(),
        }
    }
}

/// ONE question of ONE lens review, whatever lens the review is of: the pillar it belongs
/// to, the id it is named by on the wire, its title and the best practices it asks about,
/// each with the `ChoiceId` the lens names it by.
///
/// A lens review's question set is the LENS's own content, and this emulator holds that
/// content two ways: an AWS-official lens's questions are the static catalog's rows
/// ([`FrameworkQuestion`], projected by [`FrameworkQuestion::to_review_question`]), and a
/// custom lens's are the pillars and questions its imported document defines
/// ([`LensDefinition::review_questions`]). Every operation that reports or resolves a
/// question of a review speaks THIS type, so neither half of the catalog is reported
/// differently from the other.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewQuestion {
    /// `AnswerSummary$PillarId` — the pillar of the lens this question belongs to.
    pub pillar_id: String,
    /// `AnswerSummary$QuestionId` (`@length(1, 128)`), stable for the life of the lens's
    /// content.
    pub question_id: String,
    /// `AnswerSummary$QuestionTitle` (`@length(1, 512)`).
    pub title: String,
    /// The best practices this question asks about, in the lens's own order: each
    /// choice's `ChoiceId` and `Choice$Title`.
    pub best_practices: Vec<LensChoice>,
}

impl ReviewQuestion {
    /// This question's CHOICE catalog — `AnswerSummary$Choices` /
    /// `ReviewTemplateAnswerSummary$Choices` / `Answer$Choices`: one `Choice` per best
    /// practice the question asks about, in the lens's order.
    ///
    /// It is the only place a client can discover the `ChoiceId`s that
    /// `UpdateAnswer$SelectedChoices` and `ChoiceUpdates` name, so every projection of a
    /// question reports it from here and they cannot disagree. `HelpfulResource`,
    /// `ImprovementPlan` and `AdditionalResources` are left off: none of them is content
    /// this emulator holds for a question — the official catalog carries a best practice's
    /// title and the lens document format's per-choice prose is not recorded
    /// ([`parse_lens_json`]) — and every one of them is optional.
    pub fn choices(&self) -> Vec<Choice> {
        self.best_practices
            .iter()
            .map(|best_practice| Choice {
                choice_id: Some(best_practice.id.clone()),
                title: Some(best_practice.title.clone()),
                // `Choice$Description` is per-choice prose this emulator's lens content
                // does not carry, and the member is optional, so it is omitted rather
                // than filled with the title again.
                description: None,
                helpful_resource: None,
                improvement_plan: None,
                additional_resources: None,
            })
            .collect()
    }

    /// Whether `choice_id` names one of this question's choices. A choice belongs to the
    /// question that offers it, so an answer can only speak about the choices named here.
    pub fn defines_choice(&self, choice_id: &str) -> bool {
        self.each_choice_id().any(|defined| defined == choice_id)
    }

    /// The `ChoiceId` of every best practice this question asks about, in the lens's own
    /// order — the key space an answer's per-choice statuses live in, and what a
    /// request's `SelectedChoices` values and `ChoiceUpdates` keys are judged against.
    ///
    /// Lazy, so a caller that only has to RECOGNISE an id, or that consumes the ids one at
    /// a time, does not build the whole list to do it.
    fn each_choice_id(&self) -> impl Iterator<Item = &str> + '_ {
        self.best_practices
            .iter()
            .map(|best_practice| best_practice.id.as_str())
    }
}

/// The AWS Well-Architected Framework lens's question set: the content this emulator
/// carries for the lens [`FRAMEWORK_LENS_ALIAS`] names, in pillar order and then in the
/// framework's question order. It is that lens's entry in the AWS-official lens catalog
/// ([`official_lenses::OFFICIAL_LENS_CATALOG`]), which carries one question set per lens.
///
/// The pillars and the number of questions each asks are the framework's published
/// numbering (OPS 1-11, SEC 1-11, REL 1-13, PERF 1-5, COST 1-11, SUS 1-6 — 57
/// questions), and each `title` is that published question. The `question_id`s are this
/// emulator's OWN stable ids for them, `<pillar prefix>-<two-digit number>`: a lens's
/// wire question ids are lens content, which neither the Smithy model nor the service
/// docs publish, so this emulator defines them rather than guessing at AWS's. They are
/// what `QuestionId` means everywhere in this crate — `ListAnswers` reports them and an
/// answer is stored under them.
///
/// Each question's CHOICES are the best practices the Well-Architected Framework
/// publishes for it, in the framework's order; the `ChoiceId` each is reported under is
/// derived from the question and the choice's position
/// ([`FrameworkQuestion::choice_id`]). They are what a client reads to discover the
/// `ChoiceId`s `UpdateAnswer` requires, so the catalog is the one place they are defined.
pub const FRAMEWORK_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    // Operational excellence — OPS 1-11.
    q("operationalExcellence", "ops-01", "How do you determine what your priorities are?", &[
        "Evaluate external customer needs",
        "Evaluate internal customer needs",
        "Evaluate governance requirements",
        "Evaluate compliance requirements",
        "Evaluate threat landscape",
        "Evaluate tradeoffs while managing benefits and risks",
    ]),
    q("operationalExcellence", "ops-02", "How do you structure your organization to support your business outcomes?", &[
        "Resources have identified owners",
        "Processes and procedures have identified owners",
        "Operations activities have identified owners responsible for their performance",
        "Mechanisms exist to manage responsibilities and ownership",
        "Mechanisms exist to request additions, changes, and exceptions",
        "Responsibilities between teams are predefined or negotiated",
    ]),
    q("operationalExcellence", "ops-03", "How does your organizational culture support your business outcomes?", &[
        "Provide executive sponsorship",
        "Team members are empowered to take action when outcomes are at risk",
        "Escalation is encouraged",
        "Communications are timely, clear, and actionable",
        "Experimentation is encouraged",
        "Team members are encouraged to maintain and grow their skill sets",
        "Resource teams appropriately",
        "Diverse opinions are encouraged and sought within and across teams",
    ]),
    q("operationalExcellence", "ops-04", "How do you implement observability in your workload?", &[
        "Implement application telemetry",
        "Implement and configure workload telemetry",
        "Implement user experience telemetry",
        "Implement dependency telemetry",
        "Implement distributed tracing",
    ]),
    q("operationalExcellence", "ops-05", "How do you reduce defects, ease remediation, and improve flow into production?", &[
        "Use version control",
        "Test and validate changes",
        "Use configuration management systems",
        "Use build and deployment management systems",
        "Perform patch management",
        "Implement practices to improve code quality",
        "Share design standards",
        "Use multiple environments",
        "Make frequent, small, reversible changes",
        "Fully automate integration and deployment",
    ]),
    q("operationalExcellence", "ops-06", "How do you mitigate deployment risks?", &[
        "Plan for unsuccessful changes",
        "Test deployments",
        "Employ safe deployment strategies",
        "Automate testing and rollback",
    ]),
    q("operationalExcellence", "ops-07", "How do you know that you are ready to support a workload?", &[
        "Ensure personnel capability",
        "Ensure a consistent review of operational readiness",
        "Use runbooks to perform procedures",
        "Use playbooks to investigate issues",
        "Make informed decisions to deploy systems and changes",
        "Create support plans for production workloads",
    ]),
    q("operationalExcellence", "ops-08", "How do you utilize workload observability in your organization?", &[
        "Analyze workload metrics",
        "Analyze workload logs",
        "Analyze workload traces",
        "Create actionable alerts",
        "Create dashboards",
    ]),
    q("operationalExcellence", "ops-09", "How do you understand the health of your operations?", &[
        "Measure operations goals and KPIs with metrics",
        "Communicate status and trends to ensure visibility into operation",
        "Review operations metrics and prioritize improvement",
    ]),
    q("operationalExcellence", "ops-10", "How do you manage workload and operations events?", &[
        "Use a process for event, incident, and problem management",
        "Have a process per alert",
        "Prioritize operational events based on business impact",
        "Define escalation paths",
        "Define a customer communication plan for service-impacting events",
        "Communicate status through dashboards",
        "Automate responses to events",
    ]),
    q("operationalExcellence", "ops-11", "How do you evolve operations?", &[
        "Have a process for continuous improvement",
        "Perform post-incident analysis",
        "Implement feedback loops",
        "Perform knowledge management",
        "Define drivers for improvement",
        "Validate insights",
        "Perform operations metrics reviews",
        "Document and share lessons learned",
        "Allocate time to make improvements",
    ]),
    // Security — SEC 1-11.
    q("security", "sec-01", "How do you securely operate your workload?", &[
        "Separate workloads using accounts",
        "Secure account root user and properties",
        "Identify and validate control objectives",
        "Stay up to date with security threats and recommendations",
        "Reduce security management scope",
        "Automate testing and validation of security controls in pipelines",
        "Identify threats and prioritize mitigations using a threat model",
        "Evaluate and implement new security services and features regularly",
    ]),
    q("security", "sec-02", "How do you manage identities for people and machines?", &[
        "Use strong sign-in mechanisms",
        "Use temporary credentials",
        "Store and use secrets securely",
        "Rely on a centralized identity provider",
        "Audit and rotate credentials periodically",
        "Employ user groups and attributes",
    ]),
    q("security", "sec-03", "How do you manage permissions for people and machines?", &[
        "Define access requirements",
        "Grant least privilege access",
        "Establish emergency access process",
        "Reduce permissions continuously",
        "Define permission guardrails for your organization",
        "Manage access based on lifecycle",
        "Analyze public and cross-account access",
        "Share resources securely within your organization",
        "Share resources securely with a third party",
    ]),
    q("security", "sec-04", "How do you detect and investigate security events?", &[
        "Configure service and application logging",
        "Capture logs, findings, and metrics in standardized locations",
        "Correlate and enrich security alerts",
        "Initiate remediation for non-compliant resources",
    ]),
    q("security", "sec-05", "How do you protect your network resources?", &[
        "Create network layers",
        "Control traffic flow within your network layers",
        "Implement inspection-based protection",
        "Automate network protection",
    ]),
    q("security", "sec-06", "How do you protect your compute resources?", &[
        "Perform vulnerability management",
        "Provision compute from hardened images",
        "Reduce manual management and interactive access",
        "Validate software integrity",
        "Automate compute protection",
    ]),
    q("security", "sec-07", "How do you classify your data?", &[
        "Understand your data classification scheme",
        "Apply data protection controls based on data sensitivity",
        "Automate identification and classification",
        "Define scalable data lifecycle management",
    ]),
    q("security", "sec-08", "How do you protect your data at rest?", &[
        "Implement secure key management",
        "Enforce encryption at rest",
        "Automate data at rest protection",
        "Enforce access control",
        "Use mechanisms to keep people away from data",
    ]),
    q("security", "sec-09", "How do you protect your data in transit?", &[
        "Implement secure key and certificate management",
        "Enforce encryption in transit",
        "Authenticate network communications",
    ]),
    q("security", "sec-10", "How do you anticipate, respond to, and recover from incidents?", &[
        "Identify key personnel and external resources",
        "Develop incident management plans",
        "Prepare forensic capabilities",
        "Develop and test security incident response playbooks",
        "Pre-provision access",
        "Pre-deploy tools",
        "Run simulations",
        "Establish a framework for learning from incidents",
    ]),
    q("security", "sec-11", "How do you incorporate and validate the security properties of applications?", &[
        "Train for application security",
        "Automate testing throughout the development and release lifecycle",
        "Perform regular penetration testing",
        "Conduct code reviews",
        "Centralize services for packages and dependencies",
        "Deploy software programmatically",
        "Regularly assess security properties of the pipelines",
        "Build a program that embeds security ownership in workload teams",
    ]),
    // Reliability — REL 1-13.
    q("reliability", "rel-01", "How do you manage service quotas and constraints?", &[
        "Aware of service quotas and constraints",
        "Manage service quotas across accounts and Regions",
        "Accommodate fixed service quotas and constraints through architecture",
        "Monitor and manage quotas",
        "Automate quota management",
        "Ensure that a sufficient gap exists between the current quotas and the maximum usage to accommodate failover",
    ]),
    q("reliability", "rel-02", "How do you plan your network topology?", &[
        "Use highly available network connectivity for your workload public endpoints",
        "Provision redundant connectivity between private networks in the cloud and on-premises environments",
        "Ensure IP subnet allocation accounts for expansion and availability",
        "Prefer hub-and-spoke topologies over many-to-many mesh",
        "Enforce non-overlapping private IP address ranges in all private address spaces where they are connected",
    ]),
    q("reliability", "rel-03", "How do you design your workload service architecture?", &[
        "Choose how to segment your workload",
        "Build services focused on specific business domains and functionality",
        "Provide service contracts per API",
    ]),
    q("reliability", "rel-04", "How do you design interactions in a distributed system to prevent failures?", &[
        "Identify the kind of distributed system you depend on",
        "Implement loosely coupled dependencies",
        "Do constant work",
        "Make mutating operations idempotent",
    ]),
    q("reliability", "rel-05", "How do you design interactions in a distributed system to mitigate or withstand failures?", &[
        "Implement graceful degradation to transform applicable hard dependencies into soft dependencies",
        "Throttle requests",
        "Control and limit retry calls",
        "Fail fast and limit queues",
        "Set client timeouts",
        "Make services stateless where possible",
        "Implement emergency levers",
    ]),
    q("reliability", "rel-06", "How do you monitor workload resources?", &[
        "Monitor all components for the workload",
        "Define and calculate metrics",
        "Send notifications",
        "Automate responses",
        "Analytics",
        "Conduct reviews regularly",
        "Monitor end-to-end tracing of requests through your system",
    ]),
    q("reliability", "rel-07", "How do you design your workload to adapt to changes in demand?", &[
        "Use automation when obtaining or scaling resources",
        "Obtain resources upon detection of impairment to a workload",
        "Obtain resources upon detection that more resources are needed for a workload",
        "Load test your workload",
    ]),
    q("reliability", "rel-08", "How do you implement change?", &[
        "Use runbooks for standard activities such as deployment",
        "Integrate functional testing as part of your deployment",
        "Integrate resiliency testing as part of your deployment",
        "Deploy using immutable infrastructure",
        "Deploy changes with automation",
    ]),
    q("reliability", "rel-09", "How do you back up data?", &[
        "Identify and back up all data that needs to be backed up, or reproduce the data from sources",
        "Secure and encrypt backups",
        "Perform data backup automatically",
        "Perform periodic recovery of the data to verify backup integrity and processes",
    ]),
    q("reliability", "rel-10", "How do you use fault isolation to protect your workload?", &[
        "Deploy the workload to multiple locations",
        "Select the appropriate locations for your multi-location deployment",
        "Automate recovery for components constrained to a single location",
        "Use bulkhead architectures to limit scope of impact",
    ]),
    q("reliability", "rel-11", "How do you design your workload to withstand component failures?", &[
        "Monitor all components of the workload to detect failures",
        "Fail over to healthy resources",
        "Automate healing on all layers",
        "Rely on the data plane and not the control plane during recovery",
        "Use static stability to prevent bimodal behavior",
        "Send notifications when events impact availability",
        "Architect your product to meet availability targets and uptime service level agreements (SLAs)",
    ]),
    q("reliability", "rel-12", "How do you test reliability?", &[
        "Use playbooks to investigate failures",
        "Perform post-incident analysis",
        "Test functional requirements",
        "Test scaling and performance requirements",
        "Test resiliency using chaos engineering",
        "Conduct game days regularly",
    ]),
    q("reliability", "rel-13", "How do you plan for disaster recovery (DR)?", &[
        "Define recovery objectives for downtime and data loss",
        "Use defined recovery strategies to meet the recovery objectives",
        "Test disaster recovery implementation to validate the implementation",
        "Manage configuration drift at the DR site or Region",
        "Automate recovery",
    ]),
    // Performance efficiency — PERF 1-5.
    q("performance", "perf-01", "How do you select appropriate cloud resources and architecture patterns for your workload?", &[
        "Learn about and understand available cloud services and features",
        "Use guidance from your cloud provider or an appropriate partner to learn about architecture patterns and best practices",
        "Factor cost into architectural decisions",
        "Use policies and reference architectures",
        "Use benchmarking to drive architectural decisions",
        "Use a data-driven approach for architectural choices",
    ]),
    q("performance", "perf-02", "How do you select and use compute resources in your workload?", &[
        "Select the best compute options for your workload",
        "Understand the available compute configuration and features",
        "Collect compute-related metrics",
        "Configure and right-size compute resources",
        "Scale your compute resources dynamically",
        "Use optimized hardware-based compute accelerators",
    ]),
    q("performance", "perf-03", "How do you store, manage, and access data in your workload?", &[
        "Use a purpose-built data store that best supports your data access and storage requirements",
        "Evaluate available configuration options for data store",
        "Collect and record data store performance metrics",
        "Implement strategies to improve query performance in data store",
        "Implement data access patterns that utilize caching",
    ]),
    q("performance", "perf-04", "How do you select and configure networking resources in your workload?", &[
        "Understand how networking impacts performance",
        "Evaluate available networking features",
        "Choose appropriate dedicated connectivity or VPN for your workload",
        "Use load balancing to distribute traffic across multiple resources",
        "Choose network protocols to improve performance",
        "Choose your workload's location based on network requirements",
        "Optimize network configuration based on metrics",
    ]),
    q("performance", "perf-05", "What process do you use to support more performance efficiency for your workload?", &[
        "Establish key performance indicators (KPIs) to measure workload health and performance",
        "Use monitoring solutions to understand the areas where performance is most critical",
        "Define a process to improve workload performance",
        "Load test your workload",
        "Use automation to proactively remediate performance-related issues",
        "Keep your workload and services up-to-date",
        "Review metrics at regular intervals",
    ]),
    // Cost optimization — COST 1-11.
    q("costOptimization", "cost-01", "How do you implement cloud financial management?", &[
        "Establish ownership of cost optimization",
        "Establish a partnership between finance and technology",
        "Establish cloud budgets and forecasts",
        "Implement cost awareness in your organizational processes",
        "Report and notify on cost optimization",
        "Monitor cost proactively",
        "Keep up-to-date with new service releases",
        "Create a cost-aware culture",
        "Quantify business value from cost optimization",
    ]),
    q("costOptimization", "cost-02", "How do you govern usage?", &[
        "Develop policies based on your organization requirements",
        "Implement goals and targets",
        "Implement an account structure",
        "Implement groups and roles",
        "Implement cost controls",
        "Track project lifecycle",
    ]),
    q("costOptimization", "cost-03", "How do you monitor usage and cost?", &[
        "Configure detailed information sources",
        "Add organization information to cost and usage",
        "Identify cost attribution categories",
        "Establish organization metrics",
        "Configure billing and cost management tools",
        "Allocate costs based on workload metrics",
    ]),
    q("costOptimization", "cost-04", "How do you decommission resources?", &[
        "Track resources over their lifetime",
        "Implement a decommissioning process",
        "Decommission resources",
        "Decommission resources automatically",
        "Enforce data retention policies",
    ]),
    q("costOptimization", "cost-05", "How do you evaluate cost when you select services?", &[
        "Identify organization requirements for cost",
        "Analyze all components of the workload",
        "Perform a thorough analysis of each component",
        "Select software with cost-effective licensing",
        "Select components of this workload to optimize cost in line with organization priorities",
        "Perform cost analysis for different usage over time",
    ]),
    q("costOptimization", "cost-06", "How do you meet cost targets when you select resource type, size and number?", &[
        "Perform cost modeling",
        "Select resource type, size, and number based on data",
        "Select resource type, size, and number automatically based on metrics",
    ]),
    q("costOptimization", "cost-07", "How do you use pricing models to reduce cost?", &[
        "Perform pricing model analysis",
        "Choose Regions based on cost",
        "Select third-party agreements with cost-efficient terms",
        "Implement pricing models for all components of this workload",
        "Perform pricing model analysis at the management account level",
    ]),
    q("costOptimization", "cost-08", "How do you plan for data transfer charges?", &[
        "Perform data transfer modeling",
        "Select components to optimize data transfer cost",
        "Implement services to reduce data transfer costs",
    ]),
    q("costOptimization", "cost-09", "How do you manage demand, and supply resources?", &[
        "Perform an analysis on the workload demand",
        "Implement a buffer or throttle to manage demand",
        "Supply resources dynamically",
    ]),
    q("costOptimization", "cost-10", "How do you evaluate new services?", &[
        "Develop a workload review process",
        "Review and analyze this workload regularly",
    ]),
    q("costOptimization", "cost-11", "How do you evaluate the cost of effort?", &[
        "Perform automation for operations",
    ]),
    // Sustainability — SUS 1-6.
    q("sustainability", "sus-01", "How do you select Regions for your workload?", &[
        "Choose Region based on both business requirements and sustainability goals",
    ]),
    q("sustainability", "sus-02", "How do you align cloud resources to your demand?", &[
        "Scale workload infrastructure dynamically",
        "Align SLAs with sustainability goals",
        "Stop the creation and maintenance of unused assets",
        "Optimize geographic placement of workloads based on their networking requirements",
        "Optimize team member resources for activities performed",
        "Implement buffering or throttling to flatten the demand curve",
    ]),
    q("sustainability", "sus-03", "How do you take advantage of software and architecture patterns to support your sustainability goals?", &[
        "Optimize software and architecture for asynchronous and scheduled jobs",
        "Remove or refactor workload components with low or no use",
        "Optimize areas of code that consume the most time or resources",
        "Optimize impact on devices and equipment",
        "Use software patterns and architectures that best support data access and storage patterns",
    ]),
    q("sustainability", "sus-04", "How do you take advantage of data access and usage patterns to support your sustainability goals?", &[
        "Implement a data classification policy",
        "Use technologies that support data access and storage patterns",
        "Use lifecycle policies to delete unnecessary data",
        "Minimize over-provisioning in block storage",
        "Remove unneeded or redundant data",
        "Use shared file systems or storage to access common data",
        "Minimize data movement across networks",
        "Back up data only when difficult to recreate",
    ]),
    q("sustainability", "sus-05", "How do your hardware management and usage practices support your sustainability goals?", &[
        "Use the minimum amount of hardware to meet your needs",
        "Use instance types with the least impact",
        "Use managed services",
        "Optimize your use of hardware-based compute accelerators",
    ]),
    q("sustainability", "sus-06", "How do your development and deployment processes support your sustainability goals?", &[
        "Adopt methods that can rapidly introduce sustainability improvements",
        "Keep your workload up-to-date",
        "Increase utilization of build environments",
        "Use managed device farms for testing",
    ]),
];

/// One [`FrameworkQuestion`], so the catalog above reads as a table.
const fn q(
    pillar_id: &'static str,
    question_id: &'static str,
    title: &'static str,
    best_practices: &'static [&'static str],
) -> FrameworkQuestion {
    FrameworkQuestion {
        pillar_id,
        question_id,
        title,
        best_practices,
    }
}

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// workload.
pub const WORKLOAD_RESOURCE_TYPE: &str = "WORKLOAD";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// milestone.
pub const MILESTONE_RESOURCE_TYPE: &str = "MILESTONE";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// review template.
pub const REVIEW_TEMPLATE_RESOURCE_TYPE: &str = "REVIEW_TEMPLATE";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// profile.
pub const PROFILE_RESOURCE_TYPE: &str = "PROFILE";

/// `ConflictException$ResourceType` for a share of a workload. The share is a resource
/// of its own — it has its own `ShareId` — and `ShareResourceType`
/// (`smithy_query(query="shape", shape="ShareResourceType")`) names the shared resource
/// kind `WORKLOAD`, so a share of one is named here by what it is.
pub const WORKLOAD_SHARE_RESOURCE_TYPE: &str = "WORKLOAD_SHARE";

/// `ConflictException$ResourceType` for a share of a LENS — the counterpart of
/// [`WORKLOAD_SHARE_RESOURCE_TYPE`] for the other resource kind this emulator shares.
/// The share is a resource of its own (its own `ShareId`), while
/// [`LENS_RESOURCE_TYPE`] is the `ShareResourceType` member naming the shared resource
/// KIND, so the two are spelled apart for the same reason.
pub const LENS_SHARE_RESOURCE_TYPE: &str = "LENS_SHARE";

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for a
/// share invitation. The invitation is a resource of its own — it has its own
/// `ShareInvitationId`, which `UpdateShareInvitation` resolves it by — so it is named
/// here by what it is, distinctly from the share it mediates
/// ([`WORKLOAD_SHARE_RESOURCE_TYPE`]).
pub const SHARE_INVITATION_RESOURCE_TYPE: &str = "SHARE_INVITATION";

/// `ShareStatus` a newly created share carries, whatever resource it shares.
///
/// A share is mediated by an invitation — "Shared access to a workload is not removed
/// until the workload invitation is deleted" (`CreateWorkloadShare` doc), and the same
/// sentence about the lens invitation in `CreateLensShare`'s — so a share the sharer has
/// just created is waiting on the principal it was offered to, and `PENDING` is the
/// `ShareStatus` member for exactly that
/// (`smithy_query(query="shape", shape="ShareStatus")`). Nothing the CLOCK does moves it:
/// it leaves `PENDING` only when the recipient answers the invitation
/// (`UpdateShareInvitation`).
pub const DEFAULT_SHARE_STATUS: &str = "PENDING";

/// The `ShareInvitationAction` members the model publishes
/// (`smithy_query(query="shape", shape="ShareInvitationAction")`): the two answers the
/// principal a resource was shared with can give its invitation.
pub const SHARE_INVITATION_ACTION_ACCEPT: &str = "ACCEPT";
/// See [`SHARE_INVITATION_ACTION_ACCEPT`].
pub const SHARE_INVITATION_ACTION_REJECT: &str = "REJECT";

/// The `ShareStatus` a share carries once the principal it was offered to has accepted
/// its invitation, and the one it carries once that principal has rejected it — both
/// published members of the enum (`smithy_query(query="shape", shape="ShareStatus")`).
/// Each is TERMINAL: the answer is the last thing that happens to the share's status, so
/// nothing the clock does moves it afterwards.
pub const SHARE_STATUS_ACCEPTED: &str = "ACCEPTED";
/// See [`SHARE_STATUS_ACCEPTED`].
pub const SHARE_STATUS_REJECTED: &str = "REJECTED";

/// The `ShareStatus` the share mediated by an invitation moves to when that invitation
/// is answered with `action`, or `None` when `action` is not a published
/// `ShareInvitationAction` member.
///
/// One mapping, here, so every resource kind whose share invitation can be answered
/// records the same status for the same answer.
pub fn share_status_for_invitation_action(action: &str) -> Option<&'static str> {
    match action {
        SHARE_INVITATION_ACTION_ACCEPT => Some(SHARE_STATUS_ACCEPTED),
        SHARE_INVITATION_ACTION_REJECT => Some(SHARE_STATUS_REJECTED),
        _ => None,
    }
}

/// The `ShareResourceType` member that names a REVIEW TEMPLATE share
/// (`smithy_query(query="shape", shape="ShareResourceType")` publishes `WORKLOAD`,
/// `LENS`, `PROFILE` and `TEMPLATE`). Distinct from
/// [`REVIEW_TEMPLATE_RESOURCE_TYPE`], which is how an exception names the template
/// resource itself; the other three share resource types are spelled the same as the
/// exception resource types, so [`WORKLOAD_RESOURCE_TYPE`], [`LENS_RESOURCE_TYPE`] and
/// [`PROFILE_RESOURCE_TYPE`] serve for those.
pub const TEMPLATE_SHARE_RESOURCE_TYPE: &str = "TEMPLATE";

/// The most shares one workload holds in this emulator.
///
/// The Well-Architected Tool meters workload shares per workload and
/// `CreateWorkloadShare` models `ServiceQuotaExceededException` (HTTP 402, "The user has
/// reached their resource quota") for the case; the real quota is adjustable through
/// Service Quotas, so the number here is this emulator's own ceiling rather than a value
/// read off a doc page. It is what bounds the share map's growth — one share per
/// principal per workload, and every create token is 1:1 with a live share — and it sits
/// far above any realistic client workflow, so a caller never meets it by accident.
pub const MAX_SHARES_PER_WORKLOAD: usize = 1_000;

/// The most shares one lens holds in this emulator.
///
/// The same reasoning as [`MAX_SHARES_PER_WORKLOAD`], for the other resource kind that
/// is shared here: `CreateLensShare` models `ServiceQuotaExceededException` (HTTP 402,
/// "The user has reached their resource quota") for the metered case and the real quota
/// is adjustable through Service Quotas, so the number is this emulator's own ceiling
/// rather than a value read off a doc page. It bounds the lens-share map's growth — one
/// share per principal per lens, and every create token is 1:1 with a live share.
pub const MAX_SHARES_PER_LENS: usize = 1_000;

/// `ResourceNotFoundException$ResourceType` for the review of one lens in one workload
/// — the resource `ListAnswers` and the other per-lens reads resolve. The modelled
/// member is a free-form string (`smithy_query(query="shape",
/// shape="ExceptionResourceType")`), so the type names the resource that was missing:
/// the workload's review OF that lens, which exists only while the lens is associated
/// with the workload.
pub const LENS_REVIEW_RESOURCE_TYPE: &str = "LENS_REVIEW";

/// `AnswerSummary$QuestionType` for every question of a workload's lens review here.
///
/// The enum publishes exactly `PRIORITIZED` and `NON_PRIORITIZED`
/// (`smithy_query(query="shape", shape="QuestionType")`), and a question is prioritized
/// because a PROFILE associated with the workload prioritises it. WHICH lens questions a
/// profile prioritises is decided by the profile TEMPLATE's questions, which this emulator
/// does not carry (`GetProfileTemplate` is a stub with its own issue), so a profile
/// associated with a workload prioritises no question of it and every question of every
/// workload reports this type.
pub const DEFAULT_QUESTION_TYPE: &str = "NON_PRIORITIZED";

/// The `QuestionType` a question is prioritized with, the counterpart of
/// [`DEFAULT_QUESTION_TYPE`] (`smithy_query(query="shape", shape="QuestionType")`).
pub const PRIORITIZED_QUESTION_TYPE: &str = "PRIORITIZED";

/// The `QuestionPriority` that selects a question of the given `QuestionType`:
/// `PRIORITIZED` selects the prioritized questions and `NONE` the rest. The two enums
/// publish `PRIORITIZED | NONE` and `PRIORITIZED | NON_PRIORITIZED` respectively
/// (`smithy_query(query="shape", shape="QuestionPriority,QuestionType")`). One mapping,
/// here, so every operation that filters or reports a question's priority agrees.
pub fn question_priority_of(question_type: &str) -> &'static str {
    if question_type == PRIORITIZED_QUESTION_TYPE {
        "PRIORITIZED"
    } else {
        "NONE"
    }
}

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

/// The most review templates this emulator's store holds.
///
/// `CreateReviewTemplate` models `ServiceQuotaExceededException` (HTTP 402, "The user
/// has reached their resource quota") for the metered case, and the real quota is
/// adjustable through Service Quotas, so this is this emulator's own ceiling rather than
/// a value read off a doc page — the same reasoning as
/// [`MAX_WORKLOADS_PER_ACCOUNT_REGION`], and what bounds the template map and the create
/// tokens keyed to it. The count is store-wide rather than per Region: the emulator
/// serves ONE account, so a store-wide ceiling also holds for every account-and-Region
/// pair inside it, and no scan of the stored ARNs is needed to enforce it.
pub const MAX_REVIEW_TEMPLATES: usize = 10_000;

/// The most lenses one review template carries — a quota this emulator sets.
///
/// `CreateReviewTemplateInput$Lenses` targets `ReviewTemplateLenses`, which publishes no
/// `@length`, so nothing in the model bounds the list a create applies. The number here
/// is the one the service publishes for a review-template lens list elsewhere:
/// `UpdateReviewTemplate`'s `LensesToAssociate` / `LensesToDisassociate` are
/// `ReviewTemplateLensAliases`, `@length(min: 1, max: 10)`
/// (`smithy_query(query="shape", shape="ReviewTemplateLensAliases")`). Because it is an
/// emulator quota rather than a modelled constraint, `CreateReviewTemplate` answers a
/// longer list with the modelled `ServiceQuotaExceededException`, not a
/// `ValidationException`. It is what bounds a STORED template's lens list, and with it
/// the work of applying that template to a workload.
pub const MAX_LENSES_PER_REVIEW_TEMPLATE: usize = 10;

/// The most lenses one `CreateWorkload` request attaches — the same kind of quota as
/// [`MAX_LENSES_PER_REVIEW_TEMPLATE`], for the lens list of a WORKLOAD.
///
/// `CreateWorkloadInput$Lenses` targets `WorkloadLenses`, which publishes no `@length`, so
/// nothing in the model bounds the list a create attaches. The number here is the one the
/// service publishes for a workload lens list elsewhere: `AssociateLenses` /
/// `DisassociateLenses` take `LensAliases`, whose doc is "List of lens aliases to associate
/// or disassociate with a workload. Up to 10 lenses can be specified."
/// (`smithy_query(query="shape", shape="LensAliases")`). Being an emulator quota rather
/// than a modelled constraint, `CreateWorkload` answers a longer list with the modelled
/// `ServiceQuotaExceededException` — as `CreateReviewTemplate` does for a template's — and
/// it is what bounds the lens list ONE create attaches, the lens resolution that
/// create runs ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::unapplicable_lenses_refusal`])
/// and the per-lens work of every read that reports its lens reviews. What bounds the
/// list a STORED workload carries once further lenses are associated with it is the
/// service's own documented ceiling, [`MAX_LENSES_ASSOCIATED_PER_WORKLOAD`].
pub const MAX_LENSES_PER_WORKLOAD: usize = 10;

/// The most lenses one workload is reviewed against, however many requests attached them:
/// "A maximum of 20 lenses can be associated with a workload"
/// (`smithy_query(query="operation", operation="AssociateLenses")`).
///
/// A ceiling the service publishes rather than one this emulator chose, and the one bound
/// on a STORED workload's lens list: the per-request quotas above bound only what a single
/// create or association names. `AssociateLenses` models no `ServiceQuotaExceededException`,
/// so a request that would leave a workload reviewed against more lenses than this is
/// answered with the modelled `ValidationException`
/// ([`too_many_lenses_associated_exception`]). It is also what keeps the per-lens work of
/// every read that reports a workload's lens reviews bounded.
pub const MAX_LENSES_ASSOCIATED_PER_WORKLOAD: usize = 20;

/// The most lens aliases one `AssociateLenses` / `DisassociateLenses` request may name —
/// an emulator quota, like [`MAX_PROFILE_ARNS_PER_REQUEST`], because the member those two
/// operations carry the aliases in publishes only `@length(min: 1)`
/// (`smithy_query(query="shape", shape="LensAliases")`), so nothing in the model bounds
/// the list a caller sends. The number is the one that member's own doc states: "Up to 10
/// lenses can be specified".
///
/// It is what makes the per-request work of an association constant rather than
/// caller-chosen: the length is checked before any per-entry work and before the shared
/// store lock is taken, so an oversized list costs one comparison. Being a request-shape
/// rule rather than a resource quota — and neither operation models
/// `ServiceQuotaExceededException` — it is answered with the modelled `ValidationException`
/// naming [`LENS_ALIASES_MEMBER`] ([`too_many_lens_aliases_exception`]).
pub const MAX_LENS_ALIASES_PER_REQUEST: usize = 10;

/// The most profiles one workload is reviewed against — a MODELLED bound, unlike the lens
/// quota above: `WorkloadProfiles`, the list every read of a workload reports its
/// associations by, is `@length(max: 1)` (`smithy_query(query="shape",
/// shape="WorkloadProfiles")`), as is `CreateWorkloadInput$ProfileArns`. A workload holding
/// more than one could not be serialized, so an association request that would leave it
/// holding more is refused instead ([`PROFILE_ARNS_MEMBER`]).
pub const MAX_PROFILES_PER_WORKLOAD: usize = 1;

/// The modelled input member a profile-association finding names
/// (`AssociateProfilesInput$ProfileArns` / `DisassociateProfilesInput$ProfileArns`).
pub const PROFILE_ARNS_MEMBER: &str = "ProfileArns";

/// The most `ProfileArn`s one `AssociateProfiles` / `DisassociateProfiles` request may
/// name — an emulator quota, like [`MAX_LENSES_PER_WORKLOAD`], because the member those
/// two operations carry the ARNs in (`ProfileArns`) publishes only `@length(min: 1)`
/// (`smithy_query(query="shape", shape="ProfileArns")`), so nothing in the model bounds
/// the list a caller sends. The number is the one the service publishes for the list of
/// the SIBLING associate/disassociate pair: "Up to 10 lenses can be specified"
/// (`smithy_query(query="shape", shape="LensAliases")`).
///
/// It is what makes the per-request work of those operations constant rather than
/// caller-chosen: the length is checked before any per-entry work and before the shared
/// store lock is taken, so an oversized list costs one comparison. Being a request-shape
/// rule rather than a resource quota — and neither operation models
/// `ServiceQuotaExceededException` — it is answered with the modelled
/// `ValidationException` naming [`PROFILE_ARNS_MEMBER`]
/// ([`too_many_profile_arns_exception`]).
pub const MAX_PROFILE_ARNS_PER_REQUEST: usize = 10;

/// `ReviewTemplateUpdateStatus$CURRENT`: what a review template reports while every lens
/// applied to it is at its current version. The enum publishes exactly `CURRENT` and
/// `LENS_NOT_CURRENT` (`smithy_query(query="shape",
/// shape="ReviewTemplateUpdateStatus")`), and a template whose lenses have not been
/// superseded is current.
pub const REVIEW_TEMPLATE_UPDATE_STATUS_CURRENT: &str = "CURRENT";

/// `ReviewTemplateUpdateStatus$LENS_NOT_CURRENT`: what a review template reports once a
/// lens applied to it has a version later than the one its lens review is against
/// ([`ReviewTemplateLensReviewRecord::lens_version`]) — the template's own view of the
/// `LensStatus` that review reports ([`LENS_STATUS_NOT_CURRENT`]), which
/// `UpgradeReviewTemplateLensReview` is what clears.
pub const REVIEW_TEMPLATE_UPDATE_STATUS_LENS_NOT_CURRENT: &str = "LENS_NOT_CURRENT";

/// The `Question` bucket a question with no answer counts under —
/// `smithy_query(query="shape", shape="Question")` publishes exactly `UNANSWERED` and
/// `ANSWERED`. Named here because the `QuestionCounts` roll-up below fills it from the
/// lens question set rather than from an answer.
pub const QUESTION_UNANSWERED: &str = "UNANSWERED";

/// The `Question` bucket a question that CARRIES an answer counts under — the other
/// member `smithy_query(query="shape", shape="Question")` publishes. Named here
/// because the `QuestionCounts` roll-up below fills it from the stored answers.
pub const QUESTION_ANSWERED: &str = "ANSWERED";

/// `ReviewTemplateAnswerStatus$ANSWERED`: a review-template question the template
/// records an answer for. The enum publishes exactly `ANSWERED` and `UNANSWERED`
/// (`smithy_query(query="shape", shape="ReviewTemplateAnswerStatus")`), and it is the
/// stored answer — not a separate flag — that decides which one a question reports.
pub const REVIEW_TEMPLATE_ANSWER_STATUS_ANSWERED: &str = "ANSWERED";

/// `ReviewTemplateAnswerStatus$UNANSWERED`: a review-template question the template
/// records no answer for. See [`REVIEW_TEMPLATE_ANSWER_STATUS_ANSWERED`].
pub const REVIEW_TEMPLATE_ANSWER_STATUS_UNANSWERED: &str = "UNANSWERED";

/// The most milestones one workload holds: "A workload can have a maximum of 100
/// milestones" (`MilestoneNumber` doc), which is also why `MilestoneNumber` is
/// `@range(min: 1, max: 100)`. Numbers are handed out from 1 upwards and never reused,
/// so this is what bounds the milestone map per workload.
pub const MAX_MILESTONES_PER_WORKLOAD: i32 = 100;

/// The largest page most of this service's paginated operations serve, and the page size
/// a request that omits `MaxResults` gets.
///
/// Every paginated operation here targets the one modelled `MaxResults` shape, but its
/// `@range` is a MEMBER trait: all but one of them bound it to
/// `@range(min: 1, max: 50)`, so one ceiling covers them, and a request that omits the
/// member asks for a full page. The exception carries its own ceiling
/// ([`IMPROVEMENTS_MAX_RESULTS_CEILING`]).
pub const MAX_RESULTS_CEILING: i32 = 50;

/// The ceiling `ListLensReviewImprovements` serves: its `MaxResults` member is
/// `@range(min: 1, max: 100)`
/// (`smithy_query(query="operation", operation="ListLensReviewImprovements")`), twice
/// what every other paginated member of this service allows.
pub const IMPROVEMENTS_MAX_RESULTS_CEILING: i32 = 100;

/// [`MAX_RESULTS_CEILING`] applied to the `MaxResults` a request carried: the modelled
/// range restated in the handler, so the page size is in `1..=MAX_RESULTS_CEILING`
/// whatever reaches it.
pub fn page_size(max_results: Option<i32>) -> usize {
    page_size_up_to(max_results, MAX_RESULTS_CEILING)
}

/// [`page_size`] against the ceiling the CALLING operation's `MaxResults` member is
/// bounded by, for the one whose bound is not [`MAX_RESULTS_CEILING`]: the page size is
/// in `1..=ceiling` whatever reaches it, and a request that omits the member gets a full
/// page of that operation's own size.
pub fn page_size_up_to(max_results: Option<i32>, ceiling: i32) -> usize {
    let clamped = max_results.unwrap_or(ceiling).clamp(1, ceiling);
    usize::try_from(clamped)
        .expect("MaxResults is clamped into 1..=ceiling, which always fits in a usize")
}

/// One stored answer to one question of one lens review, as the operations that answer
/// a question record it.
///
/// Only the members a projection reports are held: `AnswerSummary` carries the selected
/// choices, the per-choice answers, whether the question applies and the reason it does
/// not, and `Answer` / `ReviewTemplateAnswer` carry the answer's notes too. The
/// question's RISK is not stored — it is derived from these by [`AnswerRecord::risk`] on
/// every read, so it moves when the answer does.
#[derive(Debug, Clone)]
pub struct AnswerRecord {
    /// `SelectedChoices`: the `ChoiceId`s of the best practices that are in place.
    pub selected_choices: Vec<String>,
    /// The answer recorded for each choice the answer speaks about, keyed by `ChoiceId`.
    pub choice_answers: BTreeMap<String, ChoiceAnswerRecord>,
    /// `IsApplicable`: false when the question does not apply to this workload.
    pub is_applicable: bool,
    /// `Reason` (an `AnswerReason`): why the question does not apply, when it does not.
    pub reason: Option<String>,
    /// `Notes`: the notes recorded against this answer, absent until one is written.
    pub notes: Option<String>,
}

/// One stored per-choice answer: the choice's status, the reason for it and its notes.
#[derive(Debug, Clone)]
pub struct ChoiceAnswerRecord {
    /// A `ChoiceStatus` (`SELECTED` | `UNSELECTED` | `NOT_APPLICABLE`).
    pub status: String,
    /// A `ChoiceReason`, when one was given.
    pub reason: Option<String>,
    /// `ChoiceAnswer$Notes` — "the notes associated with a choice", when any were given.
    /// `ChoiceAnswerSummary` models no notes member, so only the full answer reports it.
    pub notes: Option<String>,
}

impl AnswerRecord {
    /// The answer a question carries the moment it is first answered: every choice the
    /// question asks about recorded [`CHOICE_STATUS_UNSELECTED`] — a best practice is not
    /// in place until an answer says it is — with nothing selected, the question applying
    /// and no reason or notes.
    ///
    /// The whole choice set is recorded rather than only the choices a request speaks
    /// about, so an answered question reports one `ChoiceAnswers` entry per choice of the
    /// question however it was answered.
    pub fn for_question(question: &ReviewQuestion) -> Self {
        Self {
            selected_choices: Vec::new(),
            choice_answers: question
                .each_choice_id()
                .map(|choice_id| {
                    (
                        choice_id.to_string(),
                        ChoiceAnswerRecord {
                            status: CHOICE_STATUS_UNSELECTED.to_string(),
                            reason: None,
                            notes: None,
                        },
                    )
                })
                .collect(),
            is_applicable: true,
            reason: None,
            notes: None,
        }
    }

    /// Record `selected` as exactly the choices of `question` that are in place: each
    /// `ChoiceId` it names becomes [`CHOICE_STATUS_SELECTED`] and every OTHER choice of
    /// the question becomes [`CHOICE_STATUS_UNSELECTED`] — "the values entered replace the
    /// previously selected choices" (`SelectedChoices` doc).
    ///
    /// Only each choice's STATUS is written: a choice's reason and notes are what a
    /// `ChoiceUpdate` carries, so they are left as stored.
    pub fn select_only(&mut self, question: &ReviewQuestion, selected: &[String]) {
        let selected: BTreeSet<&str> = selected.iter().map(String::as_str).collect();
        for choice_id in question.each_choice_id() {
            let status = if selected.contains(choice_id) {
                CHOICE_STATUS_SELECTED
            } else {
                CHOICE_STATUS_UNSELECTED
            };
            self.choice_answers
                .entry(choice_id.to_string())
                .and_modify(|choice| choice.status = status.to_string())
                .or_insert_with(|| ChoiceAnswerRecord {
                    status: status.to_string(),
                    reason: None,
                    notes: None,
                });
        }
    }

    /// Bring [`Self::selected_choices`] back into agreement with the per-choice statuses:
    /// the selected choices of an answer ARE the choices of `question` it records
    /// [`CHOICE_STATUS_SELECTED`] for, in the lens's own choice order.
    ///
    /// Run after every write to an answer, so the two members a client reads back cannot
    /// contradict each other whichever of them the request wrote through.
    pub fn resolve_selected_choices(&mut self, question: &ReviewQuestion) {
        self.selected_choices = question
            .each_choice_id()
            .filter(|choice_id| {
                self.choice_answers
                    .get(*choice_id)
                    .is_some_and(|choice| choice.status == CHOICE_STATUS_SELECTED)
            })
            .map(str::to_string)
            .collect();
    }

    /// The `Risk` this answer carries, derived from what it records.
    ///
    /// The Well-Architected Tool rates a question by how many of its best practices are
    /// in place: a question that does not apply is `NOT_APPLICABLE`, one answered with
    /// no best practice selected is `HIGH` risk, one where some choice is still
    /// unselected is `MEDIUM`, and one with nothing left unselected is `NONE`.
    /// `UNANSWERED` is not reachable here — it is what a question with NO stored answer
    /// reports ([`RISK_UNANSWERED`]), which is why this takes an answer.
    pub fn risk(&self) -> &'static str {
        if !self.is_applicable {
            return RISK_NOT_APPLICABLE;
        }
        if self.selected_choices.is_empty() {
            return RISK_HIGH;
        }
        if self
            .choice_answers
            .values()
            .any(|choice| choice.status == CHOICE_STATUS_UNSELECTED)
        {
            return RISK_MEDIUM;
        }
        RISK_NONE
    }
}

/// The `Answer` projection of one question of a lens review: the question as the LENS
/// defines it, plus the answer the reviewed resource records for it.
///
/// Lives here rather than in a handler because both the read of a single answer and the
/// write that records one report it, so the two cannot disagree about what an answer
/// looks like. A question with NO stored answer is `UNANSWERED` and applicable, with
/// nothing selected and no choice answered — the same reading of an unanswered question
/// the risk roll-up counts ([`WorkloadRecord::risk_counts`]).
///
/// `QuestionDescription`, `ImprovementPlanUrl`, `HelpfulResourceUrl` and
/// `HelpfulResourceDisplayText` are omitted: each is per-question or per-choice prose
/// this emulator's lens content does not carry, and the members are optional, so they
/// are left off rather than filled with something else.
pub fn answer_projection(
    question: &ReviewQuestion,
    answer: Option<&AnswerRecord>,
) -> crate::ops::types::Answer {
    crate::ops::types::Answer {
        question_id: Some(question.question_id.clone()),
        pillar_id: Some(question.pillar_id.clone()),
        question_title: Some(question.title.clone()),
        question_description: None,
        improvement_plan_url: None,
        helpful_resource_url: None,
        helpful_resource_display_text: None,
        // The question's CHOICE CATALOG: the best practices it asks about, reported
        // whether or not the question has been answered, because it describes the
        // QUESTION. It is also the only place a client can discover the `ChoiceId`s an
        // update names.
        choices: Some(question.choices()),
        selected_choices: Some(
            answer
                .map(|answer| answer.selected_choices.clone())
                .unwrap_or_default(),
        ),
        choice_answers: Some(
            answer
                .map(|answer| {
                    answer
                        .choice_answers
                        .iter()
                        .map(|(choice_id, choice)| crate::ops::types::ChoiceAnswer {
                            choice_id: Some(choice_id.clone()),
                            status: Some(choice.status.clone()),
                            reason: choice.reason.clone(),
                            notes: choice.notes.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        // A question applies unless an answer says it does not.
        is_applicable: Some(answer.is_none_or(|answer| answer.is_applicable)),
        risk: Some(answer.map_or(RISK_UNANSWERED, AnswerRecord::risk).to_string()),
        // `Notes` and `Reason` exist only once an answer records them.
        notes: answer.and_then(|answer| answer.notes.clone()),
        reason: answer.and_then(|answer| answer.reason.clone()),
        // A question's Jira issue exists once the workload's Jira integration has synced
        // one; `UpdateIntegration` is still a stub, so nothing is synced here.
        jira_configuration: None,
    }
}

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

/// The most versions one custom lens carries: "A lens can have up to 100 versions" (the
/// `CreateLensVersion` doc). `CreateLensVersion` models `ServiceQuotaExceededException`
/// (HTTP 402, "The user has reached their resource quota"), which is what a publish past
/// this ceiling answers, so this is what bounds a lens record's version map.
pub const MAX_LENS_VERSIONS_PER_LENS: usize = 100;

/// The most idempotency tokens ONE ledger remembers.
///
/// A `ClientRequestToken` is caller-chosen and a replay of one creates no resource, so no
/// resource quota bounds a ledger keyed by it: each carries its own ceiling, evicting the
/// token remembered longest ago ([`TokenLedger::remember`]), the convention this workspace
/// uses for an idempotency ledger. A caller whose token has been forgotten gets a fresh
/// create rather than a replay, which is also what AWS's own "the result of the original
/// request is returned" window does once it closes. It sits far above any realistic client
/// workflow.
pub const MAX_REMEMBERED_REQUEST_TOKENS: usize = 10_000;

/// A BOUNDED ledger of what each `ClientRequestToken` a caller supplied resolved to.
///
/// Every idempotent operation of this service needs one, and caller input keys it — so it
/// is this type, not a bare `HashMap`, that the state struct holds: there is no `insert`
/// to reach, only [`Self::remember`], which holds the ledger at
/// [`MAX_REMEMBERED_REQUEST_TOKENS`] by forgetting the token remembered longest ago. That
/// is what keeps a stream of fresh tokens from growing the process without bound.
#[derive(Debug)]
pub struct TokenLedger<Token> {
    remembered: HashMap<String, Token>,
    /// The remembered tokens in insertion order, so the ledger evicts the one remembered
    /// longest ago first. A `HashMap` alone cannot say which that is.
    order: VecDeque<String>,
}

/// An empty ledger, whatever the token type: `#[derive(Default)]` would demand
/// `Token: Default`, which a token record has no reason to be.
impl<Token> Default for TokenLedger<Token> {
    fn default() -> Self {
        Self {
            remembered: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl<Token> TokenLedger<Token> {
    /// What `client_request_token` resolved to, if the ledger still remembers it.
    pub fn get(&self, client_request_token: &str) -> Option<&Token> {
        self.remembered.get(client_request_token)
    }

    /// Whether the ledger still remembers `client_request_token`.
    pub fn contains_key(&self, client_request_token: &str) -> bool {
        self.remembered.contains_key(client_request_token)
    }

    /// How many tokens the ledger holds — at most [`MAX_REMEMBERED_REQUEST_TOKENS`].
    pub fn len(&self) -> usize {
        self.remembered.len()
    }

    /// Whether the ledger remembers nothing.
    pub fn is_empty(&self) -> bool {
        self.remembered.is_empty()
    }

    /// Remember what `client_request_token` resolved to, within
    /// [`MAX_REMEMBERED_REQUEST_TOKENS`].
    ///
    /// Re-recording a token already held replaces its entry and evicts nothing: it names
    /// the same request. A token the ledger has forgotten is not a replay, so the next
    /// request carrying it acts afresh — which is what bounds a stream of fresh tokens,
    /// caller-chosen as they are.
    pub fn remember(&mut self, client_request_token: String, token: Token) {
        if let Some(known) = self.remembered.get_mut(&client_request_token) {
            *known = token;
            return;
        }
        while self.order.len() >= MAX_REMEMBERED_REQUEST_TOKENS {
            match self.order.pop_front() {
                Some(oldest) => {
                    self.remembered.remove(&oldest);
                }
                None => break,
            }
        }
        self.remembered
            .insert(client_request_token.clone(), token);
        self.order.push_back(client_request_token);
    }

    /// Forget every token whose record `keep` rejects — what a delete of the resource a
    /// token resolved to does to that token.
    pub fn retain<Keep: Fn(&Token) -> bool>(&mut self, keep: Keep) {
        self.remembered.retain(|_, token| keep(token));
        self.order
            .retain(|client_request_token| self.remembered.contains_key(client_request_token));
    }

    /// Forget ONE token: what a token resolving to a resource that is no longer stored
    /// does to it, so the next request carrying it acts afresh.
    pub fn forget(&mut self, client_request_token: &str) {
        self.remembered.remove(client_request_token);
        self.order.retain(|known| known != client_request_token);
    }
}

/// What a COMPLETED delete's `ClientRequestToken` resolves to: the resource the original
/// request removed, plus a fingerprint of that request's parameters so a token spent on a
/// DIFFERENT request can be told apart from a genuine retry.
///
/// A delete is the one idempotent shape whose resource is GONE by the time a retry
/// arrives, so the ledger entry — not the resource — is what the replay is served from:
/// there is nothing left to project, only the record that this exact request has already
/// executed once. That is why ONE record type serves every delete this service models,
/// where each create carries its own (its replay has to answer the created resource's
/// identifiers).
#[derive(Debug, Clone)]
pub struct CompletedDeleteToken {
    /// The identifier the original request named — the id or ARN of the resource it
    /// deleted — so a token spent on another request can report what it already resolved
    /// to.
    pub resource_id: String,
    /// [`parameter_fingerprint`] of the original request's parameters.
    pub fingerprint: String,
}

/// What a delete carrying a `ClientRequestToken` must do, before it resolves its target.
#[derive(Debug)]
pub enum DeleteReplay {
    /// The ledger does not remember the token: this is a new request, so run the delete.
    Fresh,
    /// The token names THIS request, which already completed successfully: answer its
    /// result again rather than resolving a resource the original request removed.
    Replay,
    /// The token was already spent on a request with different parameters, so this one
    /// cannot be the same request: refuse with the modelled conflict.
    Conflict(ConflictException),
}

/// How a delete carrying `client_request_token` must proceed, read off `ledger` — the
/// ledger of deletes of this kind that have already completed.
///
/// This is the `@idempotencyToken` rule the `ClientRequestToken` shape documents, applied
/// to a delete: a retry with the same token and the same parameters, after the original
/// request completed successfully, is answered with the original request's result. A
/// delete's result is its empty success, so a caller that resolves this to
/// [`DeleteReplay::Replay`] answers that success without touching the store — the retry
/// must not be a not-found just because the original request did its job.
///
/// `resource_type` and the ledger entry's [`CompletedDeleteToken::resource_id`] are what
/// the conflict names, so a token reused for another target says which resource it was
/// already spent on.
pub fn delete_replay(
    ledger: &TokenLedger<CompletedDeleteToken>,
    client_request_token: &str,
    resource_type: &str,
    fingerprint: &str,
) -> DeleteReplay {
    match ledger.get(client_request_token) {
        None => DeleteReplay::Fresh,
        Some(spent) if spent.fingerprint == fingerprint => DeleteReplay::Replay,
        Some(spent) => DeleteReplay::Conflict(idempotency_token_conflict(
            resource_type,
            spent.resource_id.clone(),
            client_request_token,
        )),
    }
}

/// What a COMPLETED upgrade's `ClientRequestToken` resolves to: the resource the original
/// request upgraded, plus a fingerprint of that request's parameters so a token spent on a
/// DIFFERENT request can be told apart from a genuine retry.
///
/// Held for the same reason [`CompletedDeleteToken`] is: an upgrade's result is its empty
/// success, so there is nothing to project and only the record that this exact request has
/// already executed once. Replaying it from the record rather than re-running it is what
/// keeps a retry from moving a lens review onto a version published since the original
/// request answered.
#[derive(Debug, Clone)]
pub struct CompletedUpgradeToken {
    /// The identifier the original request named — the ARN of the resource whose lens
    /// review it upgraded — so a token spent on another request can report what it
    /// already resolved to.
    pub resource_id: String,
    /// [`parameter_fingerprint`] of the original request's parameters.
    pub fingerprint: String,
}

/// The most tags one resource carries: `TagMap` is `@length(min: 1, max: 50)` wherever a
/// request supplies one (`smithy_query(query="operation", operation="ImportLens")`), so
/// 50 is the most a stored resource can hold too.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// One profile associated with a workload: "The profile associated with a workload"
/// (`smithy_query(query="shape", shape="WorkloadProfile")`).
///
/// A workload is reviewed against ONE VERSION of the profile — `WorkloadProfile` carries
/// `ProfileVersion` beside `ProfileArn`, and `UpgradeProfileVersion` is what moves a
/// workload to a later one — so the association records the version it is pinned at
/// rather than resolving the profile's current one on every read.
#[derive(Debug, Clone)]
pub struct WorkloadProfileRecord {
    pub profile_arn: String,
    pub profile_version: String,
}

impl WorkloadProfileRecord {
    /// The `WorkloadProfile` projection a workload read reports this association by.
    pub fn to_workload_profile(&self) -> crate::ops::types::WorkloadProfile {
        crate::ops::types::WorkloadProfile {
            profile_arn: Some(self.profile_arn.clone()),
            profile_version: Some(self.profile_version.clone()),
        }
    }
}

/// Every resource type of this service a TAG operation can name, as
/// `(ARN resource prefix, modelled `ResourceType`, the words a message calls it by)`.
///
/// "The WorkloadArn parameter can be a workload ARN, a custom lens ARN, a profile ARN, or
/// review template ARN" (`ListTagsForResource` / `TagResource` / `UntagResource` doc) —
/// the four resource types whose creates take a `TagMap` — plus the agent profile, the
/// fifth taggable resource this service's model carries (`CreateAgentProfile$tags`,
/// `smithy_query(query="operation", operation="CreateAgentProfile")`): a tag recorded on a
/// resource has to be readable back off it.
///
/// The prefixes are matched against the RESOURCE PART of the ARN
/// (`arn:<partition>:<service>:<region>:<account>:<resource>`), so `profile/` cannot also
/// match an `agent-profile/` ARN.
const TAGGABLE_RESOURCE_KINDS: &[(&str, &str, &str)] = &[
    ("workload/", WORKLOAD_RESOURCE_TYPE, "workload"),
    ("lens/", LENS_RESOURCE_TYPE, "custom lens"),
    ("profile/", PROFILE_RESOURCE_TYPE, "profile"),
    (
        "review-template/",
        REVIEW_TEMPLATE_RESOURCE_TYPE,
        "review template",
    ),
    (
        "agent-profile/",
        AGENT_PROFILE_RESOURCE_TYPE,
        "agent profile",
    ),
];

/// What a caller has WRITTEN to ONE lens review of a workload: the review's own notes,
/// the notes of each pillar of the lens under review, and the Jira question selection
/// recorded against it.
///
/// A lens review of a workload is not a resource of its own — it exists exactly while
/// that lens is associated with the workload ([`WorkloadRecord::reviews_lens`]) — so what
/// this record holds is only what a caller has written to it. Everything else a lens
/// review reports — the lens's name, version and status, its pillars and its risk roll-up
/// — is DERIVED from the lens content and the workload's stored answers by
/// [`WorkloadRecord::to_lens_review`].
///
/// The mirror of [`ReviewTemplateLensReviewRecord`], which holds the same notes for a review
/// TEMPLATE's lens review, with two differences the two shapes make: a workload's review
/// also carries a Jira question selection (`LensReview$JiraConfiguration`), and it carries
/// no `UpdatedAt` of its own, because a workload's lens review is part of the workload
/// record — so the instant the workload last changed is the instant its reviews last
/// changed ([`WorkloadRecord::updated_at`]), which is also what a milestone freezes.
#[derive(Debug, Clone, Default)]
pub struct LensReviewRecord {
    /// `LensReview$Notes`, absent until a caller writes it.
    pub notes: Option<String>,
    /// `PillarReviewSummary$Notes` per `PillarId`, holding only the pillars a caller has
    /// written notes for.
    pub pillar_notes: BTreeMap<String, String>,
    /// `LensReview$JiraConfiguration`: the pillars and questions of this review a caller
    /// has selected, absent until one is written.
    pub jira_configuration: Option<JiraSelectedQuestionConfiguration>,
}

/// The pillars ONE lens review reports a summary for, each with the `PillarName` the lens
/// content gives it where this emulator holds that content.
///
/// The pillars the lens is organized into come first, in the lens's own order
/// ([`lens_pillars`]); then any pillar the review records notes for that the lens content
/// does not name, in `PillarId` order and with no `PillarName`, so notes a caller wrote
/// are always readable back. For a lens this emulator holds content for the second group
/// is empty — an update refuses a `PillarNotes` key that lens does not define — and for a
/// lens it holds none for the first group is, because such a lens is organized into no
/// pillars HERE while the real lens is.
///
/// Shared by a workload's lens review and a review template's: both are reviews of the
/// same lens content and report their pillars the same way, so the rule lives once.
pub fn lens_review_pillars(
    lens: &str,
    pillar_notes: Option<&BTreeMap<String, String>>,
) -> Vec<(String, Option<&'static str>)> {
    let defined = lens_pillars(lens);
    let mut pillars: Vec<(String, Option<&'static str>)> = defined
        .iter()
        .map(|(pillar_id, pillar_name, _)| ((*pillar_id).to_string(), Some(*pillar_name)))
        .collect();
    if let Some(written) = pillar_notes {
        // The notes are held in a `BTreeMap`, so the extra pillars come out in `PillarId`
        // order.
        pillars.extend(
            written
                .keys()
                .filter(|pillar_id| {
                    !defined
                        .iter()
                        .any(|(defined_id, _, _)| defined_id == pillar_id)
                })
                .map(|pillar_id| (pillar_id.clone(), None)),
        );
    }
    pillars
}

/// The `PillarId`s the lens content this emulator holds organizes a lens into, EMPTY for a
/// lens it holds no content for — which is therefore organized into no pillars here, so a
/// caller's pillar identifier has nothing to be judged against.
///
/// The one place an operation that takes a `PillarId` of a lens under review resolves the
/// pillar set to judge it against.
pub fn defined_pillar_ids(lens: &str) -> BTreeSet<&'static str> {
    lens_pillars(lens)
        .iter()
        .map(|(pillar_id, _, _)| *pillar_id)
        .collect()
}

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
    /// The answers recorded against this workload's lens reviews: lens ALIAS (as
    /// [`lens_alias`] resolves it, so one entry however the request named the lens),
    /// then `QuestionId`. An answer lives on the workload rather than in a map of its
    /// own because a milestone records the workload BY VALUE: freezing the workload
    /// freezes the answers it had, which is what a milestone read reports.
    pub answers: BTreeMap<String, BTreeMap<String, AnswerRecord>>,
    /// What a caller has written to this workload's lens reviews, by lens ALIAS (as
    /// [`lens_alias`] resolves it, so one entry however the request named the lens). A
    /// lens whose review nothing has been written to carries no entry.
    ///
    /// Held on the WORKLOAD for the reason the answers above are: a milestone records the
    /// workload by value, so freezing the workload freezes what its reviews said.
    pub lens_review_records: BTreeMap<String, LensReviewRecord>,
    /// The profiles associated with this workload, in association order.
    ///
    /// The association lives on the WORKLOAD rather than in a map of its own for the
    /// reason the answers above do: a milestone records the workload by value, so
    /// freezing the workload freezes the profiles it was reviewed against. A `Vec`
    /// rather than a map because the list is at most one entry long
    /// (`WorkloadProfiles` is `@length(max: 1)`, as is
    /// `CreateWorkloadInput$ProfileArns`) and its order is the order the response
    /// reports.
    ///
    /// `AssociateProfiles` (and a `CreateWorkload` that names a profile) is what adds an
    /// entry here, through [`Self::associate_profile`]; `DisassociateProfiles` is what
    /// removes one, and `UpgradeProfileVersion` what moves the version one is pinned at.
    /// The list is held at [`MAX_PROFILES_PER_WORKLOAD`] by the operations that add to it.
    pub profiles: Vec<WorkloadProfileRecord>,
}

impl WorkloadRecord {
    /// The answer this workload records for one question of one lens, if any.
    ///
    /// `lens` is a lens as a request names it — an alias or an ARN — and resolves
    /// through [`lens_alias`], so the same answer is found by either name.
    pub fn answer(&self, lens: &str, question_id: &str) -> Option<&AnswerRecord> {
        self.answers.get(lens_alias(lens))?.get(question_id)
    }

    /// [`Self::answer`] for a caller about to WRITE it, created from the question's own
    /// choice set on the first write ([`AnswerRecord::for_question`]).
    ///
    /// Recording an answer is what moves the question out of `UNANSWERED`
    /// ([`Self::risk_counts`]), so this is the one place a workload's answer comes into
    /// existence. `lens` is a lens as a request names it, so an answer written by either
    /// spelling is the same answer ([`lens_alias`]) — the mirror of
    /// [`ReviewTemplateRecord::answer_mut`], because a workload's lens review and a review
    /// template's record an answer the same way.
    pub fn answer_mut(&mut self, lens: &str, question: &ReviewQuestion) -> &mut AnswerRecord {
        self.answers
            .entry(lens_alias(lens).to_string())
            .or_default()
            .entry(question.question_id.clone())
            .or_insert_with(|| AnswerRecord::for_question(question))
    }

    /// Whether this workload has a lens review of the named lens.
    ///
    /// A lens review exists only while that lens is associated with the workload, and a
    /// lens is the same lens however the request named it ([`lens_alias`]) — so a lens
    /// the workload does not carry has no review to report.
    pub fn reviews_lens(&self, lens: &str) -> bool {
        self.lenses
            .iter()
            .any(|attached| lens_alias(attached) == lens_alias(lens))
    }

    /// Whether a profile ARN names a profile associated with this workload.
    ///
    /// The ARN identifies the profile on its own, so the comparison is exact: an ARN
    /// agreeing on the resource part and disagreeing on the account or the Region names a
    /// DIFFERENT profile and is not associated here.
    pub fn associates_profile(&self, profile_arn: &str) -> bool {
        self.profiles
            .iter()
            .any(|associated| associated.profile_arn == profile_arn)
    }

    /// Record the association of one profile, pinned at `profile_version`, with this
    /// workload, and answer whether it added one.
    ///
    /// Idempotent in the ARN: the ARN identifies the profile on its own
    /// ([`Self::associates_profile`]), so a profile the workload already holds is left
    /// exactly as it stood — at the version the association pinned — and `false` says so.
    /// Bounding the list is the CALLER's: a workload reports at most
    /// [`MAX_PROFILES_PER_WORKLOAD`] associations, so an operation that would push past
    /// that refuses before it writes.
    pub fn associate_profile(&mut self, profile_arn: &str, profile_version: &str) -> bool {
        if self.associates_profile(profile_arn) {
            return false;
        }
        self.profiles.push(WorkloadProfileRecord {
            profile_arn: profile_arn.to_string(),
            profile_version: profile_version.to_string(),
        });
        true
    }

    /// Drop the association of one profile from this workload.
    ///
    /// Idempotent in the ARN: a workload holds a profile at most once, so this removes at
    /// most one entry and leaves the order of the rest alone. It answers nothing, because
    /// whether the workload held the profile is what [`Self::associates_profile`] is for —
    /// an operation that has to refuse an unheld profile resolves it BEFORE removing any.
    pub fn disassociate_profile(&mut self, profile_arn: &str) {
        self.profiles
            .retain(|associated| associated.profile_arn != profile_arn);
    }

    /// The `WorkloadProfiles` projection of this workload's associations, or `None` when
    /// no profile is associated: the member describes "the profile associated with a
    /// workload", and a workload with none has no profile to report rather than an empty
    /// list of them.
    ///
    /// Public because a LENS REVIEW of the workload reports the same list — "the profiles
    /// associated with the workload" (`smithy_query(query="doc", shape="LensReview",
    /// member="Profiles")`) — so both reads project it from the one place.
    pub fn profiles_projection(&self) -> Option<Vec<crate::ops::types::WorkloadProfile>> {
        (!self.profiles.is_empty()).then(|| {
            self.profiles
                .iter()
                .map(WorkloadProfileRecord::to_workload_profile)
                .collect()
        })
    }

    /// The lens reviews this workload has: every attached lens's [`lens_alias`] — the
    /// identity the lens is ONE lens under, so a lens attached under two spellings has
    /// exactly one review — beside the name the attach recorded it under, which is how a
    /// `LensAlias` of a custom lens is its ARN.
    ///
    /// The map's own key order is the list order a page over these reviews follows: it is
    /// TOTAL and stable, so a cursor carrying the last alias served resumes the
    /// enumeration without repeating or skipping a review, whatever lenses are attached
    /// or detached in between.
    pub fn lens_reviews(&self) -> BTreeMap<&str, &str> {
        let mut reviews = BTreeMap::new();
        for lens in &self.lenses {
            reviews.entry(lens_alias(lens)).or_insert(lens.as_str());
        }
        reviews
    }

    /// This workload's risk roll-up: one entry for every `Risk` the enum publishes, so
    /// a caller that reads a bucket to report review progress always finds it.
    ///
    /// `UNANSWERED` counts the questions of every lens attached to the workload that
    /// carries no answer yet; each other bucket counts the stored answers that carry
    /// that risk ([`AnswerRecord::risk`]). The map is DERIVED from the attached lenses
    /// and the stored answers on every read and never stored, so it moves when either
    /// does.
    pub fn risk_counts(&self) -> BTreeMap<String, i32> {
        let mut counts = empty_risk_counts();
        // A lens is attached once however many ways the request named it, so count the
        // questions of each lens the workload reviews once ([`Self::lens_reviews`] is the
        // one place that identity rule lives).
        for lens in self.lens_reviews().keys() {
            for (risk, count) in self.lens_risk_counts(lens) {
                counts
                    .entry(risk)
                    .and_modify(|total| *total = total.saturating_add(count))
                    .or_insert(count);
            }
        }
        counts
    }

    /// The risk roll-up of ONE lens review of this workload: the same buckets as
    /// [`Self::risk_counts`], counted over the questions that ONE lens asks rather than
    /// over every attached lens's.
    ///
    /// `lens` is a lens as a request names it (an alias or an ARN). A lens this emulator
    /// holds no question set for asks nothing ([`lens_questions`]), so every bucket is 0.
    pub fn lens_risk_counts(&self, lens: &str) -> BTreeMap<String, i32> {
        self.risk_counts_over(lens, lens_questions(lens).iter())
    }

    /// The risk roll-up of ONE pillar of one lens review: [`Self::lens_risk_counts`]
    /// narrowed to the questions that pillar asks, which is what a pillar review summary
    /// of a lens review reports.
    pub fn pillar_risk_counts(&self, lens: &str, pillar_id: &str) -> BTreeMap<String, i32> {
        self.risk_counts_over(
            lens,
            lens_questions(lens)
                .iter()
                .filter(|question| question.pillar_id == pillar_id),
        )
    }

    /// The risk roll-up over an arbitrary set of ONE lens's questions — the one place a
    /// risk is counted, so the workload's roll-up, a lens review's and a pillar's cannot
    /// disagree about what a question counts as.
    fn risk_counts_over<'question>(
        &self,
        lens: &str,
        questions: impl Iterator<Item = &'question FrameworkQuestion>,
    ) -> BTreeMap<String, i32> {
        let mut counts = empty_risk_counts();
        for question in questions {
            let risk = match self.answer(lens, question.question_id) {
                Some(answer) => answer.risk(),
                None => RISK_UNANSWERED,
            };
            counts
                .entry(risk.to_string())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
        counts
    }

    /// What a caller has written to ONE of this workload's lens reviews, if anything has
    /// been.
    ///
    /// `lens` is a lens as a request names it — an alias or an ARN — and resolves through
    /// [`lens_alias`], so the same record is found by either name.
    pub fn lens_review_record(&self, lens: &str) -> Option<&LensReviewRecord> {
        self.lens_review_records.get(lens_alias(lens))
    }

    /// [`Self::lens_review_record`] for a caller about to WRITE it, created empty on the
    /// first write: the lens review itself exists as soon as the lens is associated with
    /// the workload, and this record is only how the store remembers what was written to
    /// it.
    pub fn lens_review_record_mut(&mut self, lens: &str) -> &mut LensReviewRecord {
        self.lens_review_records
            .entry(lens_alias(lens).to_string())
            .or_default()
    }

    /// The pillars ONE lens review of this workload reports a summary for, through the
    /// one place that rule lives ([`lens_review_pillars`]).
    pub fn lens_review_pillars(&self, lens: &str) -> Vec<(String, Option<&'static str>)> {
        lens_review_pillars(
            lens,
            self.lens_review_record(lens)
                .map(|written| &written.pillar_notes),
        )
    }

    /// The `LensReview` shape `GetLensReview` and `UpdateLensReview` return for ONE lens
    /// associated with this workload.
    ///
    /// `lens` is the lens AS THE REQUEST NAMED IT — `LensAlias` is "either the lens alias
    /// … or the lens ARN" (`LensAlias` doc) and this shape models no member to resolve one
    /// spelling into the other — and `lens_arn` is the ARN that lens is reported by
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_arn_of`]).
    ///
    /// Everything but what a caller wrote to the review ([`LensReviewRecord`]) is DERIVED
    /// on every read: the lens's own name, version and status from the lens content this
    /// emulator holds ([`lens_metadata`], absent for a lens it holds none for), one pillar
    /// summary per pillar of the lens ([`Self::lens_review_pillars`]) and the risk
    /// roll-ups of the lens's and each pillar's questions from this workload's stored
    /// answers — so a review moves when an answer does.
    pub fn to_lens_review(&self, lens: &str, lens_arn: String) -> LensReview {
        let metadata = lens_metadata(lens);
        let written = self.lens_review_record(lens);
        LensReview {
            lens_alias: Some(lens.to_string()),
            lens_arn: Some(lens_arn),
            lens_version: metadata.map(|metadata| metadata.version.to_string()),
            lens_name: metadata.map(|metadata| metadata.name.to_string()),
            lens_status: metadata.map(|metadata| metadata.status.to_string()),
            pillar_review_summaries: Some(
                self.lens_review_pillars(lens)
                    .into_iter()
                    .map(|(pillar_id, pillar_name)| PillarReviewSummary {
                        notes: written
                            .and_then(|written| written.pillar_notes.get(&pillar_id).cloned()),
                        risk_counts: Some(risk_counts_to_map(
                            &self.pillar_risk_counts(lens, &pillar_id),
                        )),
                        // The risk counts restricted to the questions a PROFILE
                        // prioritises are not projected for a pillar: which questions
                        // those are is decided by the profile template this emulator does
                        // not carry (see [`DEFAULT_QUESTION_TYPE`]), so no question of any
                        // workload is prioritized.
                        prioritized_risk_counts: None,
                        pillar_id: Some(pillar_id),
                        pillar_name: pillar_name.map(str::to_string),
                    })
                    .collect(),
            ),
            // The Jira question selection recorded against this review, absent until a
            // caller records one.
            jira_configuration: written.and_then(|written| written.jira_configuration.clone()),
            // "The date and time when the lens review was last updated": a review's
            // content — the answers recorded against it and its notes — is part of the
            // workload record, so the instant the workload last changed is the instant its
            // reviews last changed. A milestone read reports the instant the RECORDED
            // workload carried, which is what that milestone froze.
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            notes: written.and_then(|written| written.notes.clone()),
            // The roll-up over THIS lens's questions alone, derived from the stored
            // answers on every read: it moves when an answer does.
            risk_counts: Some(risk_counts_to_map(&self.lens_risk_counts(lens))),
            // Neither operation that returns this shape is `@paginated`, and both report
            // every pillar of the lens in one answer, so there is never a further page to
            // fetch.
            next_token: None,
            // "The profiles associated with the workload" — the same list the workload
            // itself reports, projected from the one place ([`Self::profiles_projection`]),
            // so a review and a read of the workload cannot disagree about which profile
            // it is reviewed against.
            profiles: self.profiles_projection(),
            prioritized_risk_counts: None,
        }
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
            // Only the workload AS THE RECIPIENT SEES IT carries these: they describe
            // the share invitation the recipient was offered, so they belong to a
            // workload ANOTHER account owns and shared in. Every request this emulator
            // serves is attributed to its one account, which owns every workload this
            // store holds — `CreateWorkloadShare` is what records a share and it refuses
            // a `SharedWith` naming the sharer — so no workload here is one the caller
            // reached through an invitation it accepted.
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
            // The profiles this workload is reviewed against, and the risk counts
            // restricted to the questions a profile prioritises. The associations are
            // projected from the record; the prioritised roll-up is NOT, because which
            // questions a profile prioritises is read off that profile's own answered
            // profile questions and this emulator holds no profile question catalog to
            // read them against (see [`DEFAULT_QUESTION_TYPE`]) — so no question of any
            // workload is prioritized and there is no prioritised roll-up to report.
            profiles: self.profiles_projection(),
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
            // As in [`Self::to_workload`]: the associations are projected from the
            // record, the prioritised roll-up has nothing to compute it from.
            profiles: self.profiles_projection(),
            prioritized_risk_counts: None,
        }
    }
}

/// A risk roll-up with one entry for every `Risk` the enum publishes, all zero: what
/// every roll-up here starts from, so a caller that reads a bucket to report review
/// progress always finds it.
fn empty_risk_counts() -> BTreeMap<String, i32> {
    RISK_VALUES
        .iter()
        .map(|risk| ((*risk).to_string(), 0))
        .collect()
}

/// A question roll-up with one entry for every `Question` the enum publishes, all zero:
/// what every `QuestionCounts` roll-up here starts from, so a caller that reads a bucket
/// to report review progress always finds it — including for a lens this emulator holds
/// no question set for, whose every bucket stays 0.
fn empty_question_counts() -> BTreeMap<String, i32> {
    QUESTION_VALUES
        .iter()
        .map(|bucket| ((*bucket).to_string(), 0))
        .collect()
}

/// A risk roll-up as the modelled `RiskCounts` map a response carries.
pub fn risk_counts_to_map(counts: &BTreeMap<String, i32>) -> HashMap<String, i32> {
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

    /// The `MilestoneSummary` projection `ListMilestones` pages over: the same milestone,
    /// with the recorded workload projected to its `WorkloadSummary`
    /// (`smithy_query(query="shape", shape="MilestoneSummary")`).
    pub fn to_summary(&self) -> MilestoneSummary {
        MilestoneSummary {
            milestone_number: Some(self.milestone_number),
            milestone_name: Some(self.milestone_name.clone()),
            recorded_at: Some(::aws_smithy_types::DateTime::from(self.recorded_at)),
            workload_summary: Some(self.workload.to_summary()),
        }
    }
}

/// One lens review of a review template, as the store holds it.
///
/// A lens review of a template is not a resource of its own — it exists exactly while
/// the lens is applied to the template ([`ReviewTemplateRecord::applies_lens`]) — so what
/// this record holds is the review's own state and nothing the lens or the template's
/// answers already say: the notes a caller has written ("the notes that will be
/// associated with the workload when the template is applied",
/// `UpdateReviewTemplateLensReviewInput$LensNotes` / `$PillarNotes` docs), when the review
/// was last written (`ReviewTemplateLensReview$UpdatedAt`, "the date and time when the
/// review template lens review was last updated") and the VERSION of the lens the review
/// is against — pinned when the lens was applied, exactly as a workload's profile
/// association pins the profile version it is reviewed against
/// ([`WorkloadProfileRecord::profile_version`]).
///
/// Everything else a lens review reports — the lens's name and currency, its pillars and
/// its question roll-up — is DERIVED from the lens catalog and the template's stored
/// answers by [`ReviewTemplateRecord::to_lens_review`].
#[derive(Debug, Clone)]
pub struct ReviewTemplateLensReviewRecord {
    /// `ReviewTemplateLensReview$Notes`, absent until a caller writes it.
    pub notes: Option<String>,
    /// `ReviewTemplatePillarReviewSummary$Notes` per `PillarId`, holding only the
    /// pillars a caller has written notes for.
    pub pillar_notes: BTreeMap<String, String>,
    /// `ReviewTemplateLensReview$UpdatedAt`.
    pub updated_at: SystemTime,
    /// The `LensVersion` this review is against: the version the lens's content was
    /// published as when the lens was applied ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::applied_lens_version`]),
    /// which `UpgradeReviewTemplateLensReview` moves to the lens's latest version.
    /// `None` for a lens this emulator holds no content for, which has no version to be
    /// at.
    pub lens_version: Option<String>,
}

/// What the lens catalog says about the lens ONE lens review is a review of, as that
/// review reports it: the ARN the lens is reported by, the `LensVersion` the review is
/// against, the lens's name as of that version and the currency of that version.
///
/// It is resolved from the catalog in ONE step
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens`])
/// and owns every value it carries, so a handler can resolve it BEFORE it takes the
/// resource carrying the review for writing and still project the review afterwards —
/// the same reason [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_arn_of`]
/// is called before the write.
///
/// Every member but the ARN is absent for a lens this emulator holds no content for: such
/// a lens has no version here to be at, and inventing one would mean reporting a review
/// against a version nothing published.
#[derive(Debug, Clone)]
pub struct ReviewedLens {
    /// `ReviewTemplateLensReview$LensArn` / `LensReview$LensArn`.
    pub lens_arn: String,
    /// `ReviewTemplateLensReview$LensVersion` / `LensReview$LensVersion` — the version
    /// the review is against, which is the lens's latest for a review that pins none.
    pub lens_version: Option<String>,
    /// `ReviewTemplateLensReview$LensName` / `LensReview$LensName`, as the content of
    /// that version names the lens.
    pub lens_name: Option<String>,
    /// `ReviewTemplateLensReview$LensStatus` / `LensReview$LensStatus`:
    /// [`LENS_STATUS_CURRENT`] while the review is against the lens's latest version and
    /// [`LENS_STATUS_NOT_CURRENT`] once a later one has been published.
    pub lens_status: Option<String>,
}

/// One stored review template: "A review template" (`ReviewTemplate` shape doc).
///
/// Every member of the modelled `ReviewTemplate` a request can set is persisted here;
/// the members the SERVICE computes — the question roll-up, the owning account and the
/// update status — are derived by [`ReviewTemplateRecord::to_review_template`], the only
/// place a response is built from this record.
#[derive(Debug, Clone)]
pub struct ReviewTemplateRecord {
    pub template_arn: String,
    pub template_name: String,
    pub description: String,
    pub notes: Option<String>,
    pub lenses: Vec<String>,
    pub owner: String,
    pub updated_at: SystemTime,
    pub tags: BTreeMap<String, String>,
    /// The answers recorded against this template's lens reviews: lens ALIAS (as
    /// [`lens_alias`] resolves it, so one entry however the request named the lens),
    /// then `QuestionId`. Held exactly as a workload holds its own
    /// ([`WorkloadRecord::answers`]) — a review template is reviewed against the same
    /// lens question set, so the two cannot disagree about what an answer records.
    pub answers: BTreeMap<String, BTreeMap<String, AnswerRecord>>,
    /// This template's lens reviews, by lens ALIAS (as [`lens_alias`] resolves it, so one
    /// entry however the request named the lens). An entry exists for every lens applied
    /// to the template, because the review exists as soon as the lens is applied
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::applied_lens_reviews`]).
    pub lens_reviews: BTreeMap<String, ReviewTemplateLensReviewRecord>,
}

impl ReviewTemplateRecord {
    /// The answer this template records for one question of one lens, if any.
    ///
    /// `lens` is a lens as a request names it — an alias or an ARN — and resolves
    /// through [`lens_alias`], so the same answer is found by either name.
    pub fn answer(&self, lens: &str, question_id: &str) -> Option<&AnswerRecord> {
        self.answers.get(lens_alias(lens))?.get(question_id)
    }

    /// [`Self::answer`] for a caller about to WRITE it, created from the question's own
    /// choice set on the first write ([`AnswerRecord::for_question`]).
    ///
    /// Recording an answer is what makes the question `ANSWERED`
    /// ([`Self::question_counts`]), so this is the one place a template's answer comes
    /// into existence. `lens` is a lens as a request names it, so an answer written by
    /// either spelling is the same answer ([`lens_alias`]).
    pub fn answer_mut(&mut self, lens: &str, question: &ReviewQuestion) -> &mut AnswerRecord {
        self.answers
            .entry(lens_alias(lens).to_string())
            .or_default()
            .entry(question.question_id.clone())
            .or_insert_with(|| AnswerRecord::for_question(question))
    }

    /// Whether this template applies the lens a `LensAlias` names, however the template
    /// and the request spell it: a lens review of a template exists only while that
    /// lens is applied to it.
    pub fn applies_lens(&self, lens: &str) -> bool {
        self.lenses
            .iter()
            .any(|applied| lens_alias(applied) == lens_alias(lens))
    }
    /// This template's question roll-up: one entry for every `Question` the enum
    /// publishes, so a caller reading a bucket to report review progress always finds
    /// it.
    ///
    /// `ANSWERED` counts the questions of the applied lenses this template records an
    /// answer for and `UNANSWERED` the rest; a lens is applied once however many ways
    /// the request named it. The map is DERIVED from the applied lenses and the stored
    /// answers on every read and never stored, so it moves when either does — exactly
    /// as a workload's risk roll-up does ([`WorkloadRecord::risk_counts`]).
    pub fn question_counts(&self) -> BTreeMap<String, i32> {
        let mut counts = empty_question_counts();
        let applied: std::collections::BTreeSet<&str> =
            self.lenses.iter().map(|lens| lens_alias(lens)).collect();
        for lens in applied {
            for (bucket, count) in self.lens_question_counts(lens) {
                counts
                    .entry(bucket)
                    .and_modify(|total| *total = total.saturating_add(count))
                    .or_insert(count);
            }
        }
        counts
    }

    /// The question roll-up of ONE lens review of this template: the same buckets as
    /// [`Self::question_counts`], counted over the questions that ONE lens asks rather
    /// than over every applied lens's — which is what a lens review reports.
    ///
    /// `lens` is a lens as a request names it (an alias or an ARN). A lens this emulator
    /// holds no question set for asks nothing ([`lens_questions`]), so every bucket is 0.
    pub fn lens_question_counts(&self, lens: &str) -> BTreeMap<String, i32> {
        self.question_counts_over(lens, lens_questions(lens).iter())
    }

    /// [`Self::lens_question_counts`] narrowed to the questions ONE pillar of that lens
    /// asks, which is what a pillar review summary of a lens review reports.
    pub fn pillar_question_counts(&self, lens: &str, pillar_id: &str) -> BTreeMap<String, i32> {
        self.question_counts_over(
            lens,
            lens_questions(lens)
                .iter()
                .filter(|question| question.pillar_id == pillar_id),
        )
    }

    /// The question roll-up over an arbitrary set of ONE lens's questions — the one
    /// place a template question is bucketed, so the template's roll-up, a lens
    /// review's and a pillar's cannot disagree about what a question counts as.
    fn question_counts_over<'question>(
        &self,
        lens: &str,
        questions: impl Iterator<Item = &'question FrameworkQuestion>,
    ) -> BTreeMap<String, i32> {
        let mut counts = empty_question_counts();
        for question in questions {
            let bucket = if self.answer(lens, question.question_id).is_some() {
                QUESTION_ANSWERED
            } else {
                QUESTION_UNANSWERED
            };
            counts
                .entry(bucket.to_string())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
        counts
    }

    /// ONE of this template's lens reviews as the store holds it, if the store holds one.
    ///
    /// `lens` is a lens as a request names it, so the same review is found by either
    /// spelling ([`lens_alias`]).
    pub fn lens_review_record(&self, lens: &str) -> Option<&ReviewTemplateLensReviewRecord> {
        self.lens_reviews.get(lens_alias(lens))
    }

    /// [`Self::lens_review_record`] for a caller about to WRITE to it, created empty at
    /// `now` if the store holds none: the lens review itself exists as soon as the lens is
    /// applied, and this record is only how the store remembers that review's own state.
    pub fn lens_review_record_mut(
        &mut self,
        lens: &str,
        now: SystemTime,
    ) -> &mut ReviewTemplateLensReviewRecord {
        self.lens_reviews
            .entry(lens_alias(lens).to_string())
            .or_insert_with(|| ReviewTemplateLensReviewRecord {
                notes: None,
                pillar_notes: BTreeMap::new(),
                updated_at: now,
                lens_version: None,
            })
    }

    /// The `LensVersion` ONE lens review of this template is against, as it was pinned
    /// when the lens was applied or last moved by an upgrade.
    pub fn lens_review_version(&self, lens: &str) -> Option<&str> {
        self.lens_review_record(lens)?.lens_version.as_deref()
    }

    /// The pillars ONE lens review of this template reports a summary for, through the
    /// one place that rule lives ([`lens_review_pillars`]) — the same rule a WORKLOAD's
    /// lens review follows ([`WorkloadRecord::lens_review_pillars`]).
    pub fn lens_review_pillars(&self, lens: &str) -> Vec<(String, Option<&'static str>)> {
        lens_review_pillars(
            lens,
            self.lens_review_record(lens)
                .map(|written| &written.pillar_notes),
        )
    }

    /// The `ReviewTemplateLensReview` shape `GetReviewTemplateLensReview` and
    /// `UpdateReviewTemplateLensReview` return for ONE lens applied to this template.
    ///
    /// `lens` is the lens AS THE REQUEST NAMED IT — `LensAlias` is "either the lens
    /// alias … or the lens ARN" (`LensAlias` doc) and this shape models no member to
    /// resolve one spelling into the other — and `reviewed` is what the lens catalog says
    /// about the lens this review is against, resolved for the version the review is at
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens`]).
    ///
    /// Everything but the review's own state is DERIVED on every read: the lens's name,
    /// version and currency from `reviewed`, one pillar summary per pillar of the lens
    /// ([`Self::lens_review_pillars`]) and the answered/unanswered roll-ups from the
    /// template's stored answers.
    pub fn to_lens_review(&self, lens: &str, reviewed: &ReviewedLens) -> ReviewTemplateLensReview {
        let written = self.lens_review_record(lens);
        ReviewTemplateLensReview {
            lens_alias: Some(lens.to_string()),
            lens_arn: Some(reviewed.lens_arn.clone()),
            lens_version: reviewed.lens_version.clone(),
            lens_name: reviewed.lens_name.clone(),
            lens_status: reviewed.lens_status.clone(),
            pillar_review_summaries: Some(
                self.lens_review_pillars(lens)
                    .into_iter()
                    .map(|(pillar_id, pillar_name)| ReviewTemplatePillarReviewSummary {
                        notes: written
                            .and_then(|written| written.pillar_notes.get(&pillar_id).cloned()),
                        question_counts: Some(
                            self.pillar_question_counts(lens, &pillar_id)
                                .into_iter()
                                .collect::<HashMap<String, i32>>(),
                        ),
                        pillar_id: Some(pillar_id),
                        pillar_name: pillar_name.map(str::to_string),
                    })
                    .collect(),
            ),
            // "The date and time when the review template lens review was last updated":
            // the review's own record carries that instant, and one the store holds no
            // record for was last changed when the template carrying it was.
            updated_at: Some(::aws_smithy_types::DateTime::from(
                written.map_or(self.updated_at, |written| written.updated_at),
            )),
            notes: written.and_then(|written| written.notes.clone()),
            question_counts: Some(
                self.lens_question_counts(lens)
                    .into_iter()
                    .collect::<HashMap<String, i32>>(),
            ),
            // `ReviewTemplateLensReview$NextToken` continues a paged read, and neither
            // operation that returns this shape models a paging member to continue, so
            // there is nothing to hand back.
            next_token: None,
        }
    }

    /// The `ReviewTemplateSummary` shape `ListReviewTemplates` reports for this template:
    /// "Summary of a review template" — the members of the stored template a LIST reports,
    /// projected from the same state [`Self::to_review_template`] projects the full shape
    /// from, so a summary and a `GetReviewTemplate` of the same template cannot disagree.
    ///
    /// `ReviewTemplateSummary` models no `Notes`, `Tags`, `QuestionCounts` or
    /// `ShareInvitationId` member (`smithy_query(query="shape",
    /// shape="ReviewTemplateSummary")`), so a summary carries only what it declares.
    ///
    /// `update_status` is this template's `ReviewTemplateUpdateStatus`, which depends on
    /// the lens catalog rather than on the record
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::review_template_update_status`]).
    pub fn to_summary(&self, update_status: &str) -> ReviewTemplateSummary {
        ReviewTemplateSummary {
            description: Some(self.description.clone()),
            lenses: Some(self.lenses.clone()),
            owner: Some(self.owner.clone()),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            template_arn: Some(self.template_arn.clone()),
            template_name: Some(self.template_name.clone()),
            update_status: Some(update_status.to_string()),
        }
    }

    /// The `ReviewTemplate` shape `GetReviewTemplate` returns. `update_status` is as
    /// [`Self::to_summary`] takes it, so a summary and a `GetReviewTemplate` of the same
    /// template cannot disagree about it.
    pub fn to_review_template(&self, update_status: &str) -> ReviewTemplate {
        ReviewTemplate {
            description: Some(self.description.clone()),
            lenses: Some(self.lenses.clone()),
            notes: self.notes.clone(),
            question_counts: Some(
                self.question_counts()
                    .into_iter()
                    .collect::<HashMap<String, i32>>(),
            ),
            owner: Some(self.owner.clone()),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            template_arn: Some(self.template_arn.clone()),
            template_name: Some(self.template_name.clone()),
            // The tag map the template carries, empty when it carries none: the
            // `@length(min: 1)` on `TagMap` constrains the tags a REQUEST supplies, not
            // what a projection of the stored template reports.
            tags: Some(self.tags.clone().into_iter().collect()),
            update_status: Some(update_status.to_string()),
            // "The ID assigned to the template share invitation": only a template reached
            // through a share invitation carries one, and template shares are still a
            // stub.
            share_invitation_id: None,
        }
    }
}

/// The `ReviewTemplateAnswer` shape `UpdateReviewTemplateAnswer` and
/// `GetReviewTemplateAnswer` report for ONE question of ONE lens review of a review
/// template: "an answer of the question".
///
/// `question` is the question as the LENS defines it and `answer` the answer the template
/// records for it, absent for a question nothing has answered — which is what makes the
/// difference between the two states a `ReviewTemplateAnswerStatus` publishes. A question
/// with no stored answer applies, has nothing selected and speaks about no choice, exactly
/// as the template's question roll-up counts it ([`ReviewTemplateRecord::question_counts`])
/// and as a `ReviewTemplateAnswerSummary` of the same question reports it.
///
/// `QuestionDescription`, `ImprovementPlanUrl`, `HelpfulResourceUrl` and
/// `HelpfulResourceDisplayText` are lens CONTENT this emulator's official-lens catalog
/// does not carry, and every one of them is optional, so they are omitted rather than
/// guessed at.
pub fn review_template_answer(
    question: &ReviewQuestion,
    answer: Option<&AnswerRecord>,
) -> ReviewTemplateAnswer {
    ReviewTemplateAnswer {
        question_id: Some(question.question_id.clone()),
        pillar_id: Some(question.pillar_id.clone()),
        question_title: Some(question.title.clone()),
        question_description: None,
        improvement_plan_url: None,
        helpful_resource_url: None,
        helpful_resource_display_text: None,
        // The per-question CHOICE CATALOG the lens defines: the best practices the
        // question asks about, each with the `ChoiceId` a client passes back in
        // `UpdateReviewTemplateAnswer$SelectedChoices` and `$ChoiceUpdates`. It describes
        // the QUESTION, so it is reported whether or not the template has answered it.
        choices: Some(question.choices()),
        selected_choices: Some(
            answer
                .map(|answer| answer.selected_choices.clone())
                .unwrap_or_default(),
        ),
        choice_answers: Some(
            answer
                .map(|answer| {
                    answer
                        .choice_answers
                        .iter()
                        .map(|(choice_id, choice)| ChoiceAnswer {
                            choice_id: Some(choice_id.clone()),
                            status: Some(choice.status.clone()),
                            reason: choice.reason.clone(),
                            notes: choice.notes.clone(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
        ),
        // A question applies unless an answer says it does not.
        is_applicable: Some(answer.is_none_or(|answer| answer.is_applicable)),
        // "The status of whether or not this question has been answered": the stored
        // answer IS that status, so an answer and the template's roll-up cannot disagree.
        answer_status: Some(
            if answer.is_some() {
                REVIEW_TEMPLATE_ANSWER_STATUS_ANSWERED
            } else {
                REVIEW_TEMPLATE_ANSWER_STATUS_UNANSWERED
            }
            .to_string(),
        ),
        notes: answer.and_then(|answer| answer.notes.clone()),
        reason: answer.and_then(|answer| answer.reason.clone()),
    }
}

/// One choice a lens question offers: "choices" in the lens JSON format specification —
/// the `ChoiceId` a client passes back when it answers the question, and the
/// `Choice$Title` the question reports for it.
///
/// It is also how a [`ReviewQuestion`] carries a best practice, whichever half of the
/// catalog the lens comes from: an official lens's choice id is derived from the question
/// and the practice's position ([`FrameworkQuestion::choice_id`]), a custom lens's is the
/// one its document names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LensChoice {
    pub id: String,
    pub title: String,
}

/// One rule that maps the choices selected for a question to the risk the answer
/// carries. `condition` is the format specification's own expression over the
/// question's choice ids, kept VERBATIM: this store records the rule rather than
/// interpreting it, and an answer's `Risk` is derived from the best practices it leaves
/// out ([`AnswerRecord::risk`]).
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

    /// The questions a review of this lens asks, in the document's own order: pillar by
    /// pillar, each pillar's questions in the order it lists them, each carrying the
    /// pillar it belongs to and the choices the document defines for it.
    ///
    /// This is the custom half of a lens review's question set — the counterpart of
    /// [`FrameworkQuestion::to_review_question`] for the AWS-official catalog — so a
    /// review of an imported lens reports the content its document supplies rather than
    /// nothing.
    pub fn review_questions(&self) -> Vec<ReviewQuestion> {
        self.pillars
            .iter()
            .flat_map(|pillar| {
                pillar.questions.iter().map(|question| ReviewQuestion {
                    pillar_id: pillar.id.clone(),
                    question_id: question.id.clone(),
                    title: question.title.clone(),
                    best_practices: question.choices.clone(),
                })
            })
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

/// One PUBLISHED version of a custom lens: the lens's content as it stood when
/// `CreateLensVersion` published that version, plus what the publishing request said
/// about it.
///
/// A version is immutable — "Create a new lens version… after you have imported a lens"
/// (`CreateLensVersion` doc) — so the content is held BY VALUE here: a later import that
/// changes the lens's draft must not change what a published version carries.
#[derive(Debug, Clone)]
pub struct PublishedLensVersion {
    /// The content published under this version.
    pub content: LensContent,
    /// `CreateLensVersionInput$IsMajorVersion` — "Set to true if this new major lens
    /// version". A request that omits the member is not publishing a major version.
    pub is_major_version: bool,
    pub published_at: SystemTime,
}

/// One stored custom lens.
///
/// A lens imported by `ImportLens` "will have a status of DRAFT. The lens cannot be
/// applied to workloads or shared with other Amazon Web Services accounts until it's
/// published with CreateLensVersion" (`ImportLens` doc) — so a record carries its draft
/// content plus the versions `CreateLensVersion` has published from it, and the latter
/// stays empty until it publishes one.
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
    pub versions: BTreeMap<String, PublishedLensVersion>,
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

    /// The version this lens's CURRENT content is published as, as `(LensVersion, that
    /// published version)`, or `None` while that content is a draft nothing has been
    /// published from — which is the lens being in DRAFT.
    ///
    /// "The new or updated lens will have a status of DRAFT. The lens cannot be applied to
    /// workloads or shared with other Amazon Web Services accounts until it's published
    /// with CreateLensVersion" (`ImportLens` doc), so the question is whether a version was
    /// published FROM the content the lens now carries: a version qualifies when it was
    /// published no earlier than that content was imported (publishing copies the content
    /// current at that instant) AND carries that same document. The instant is what makes
    /// an UPDATE a draft again even when the document it imports is byte-identical to a
    /// published one; the document is what keeps a version published before a still
    /// in-flight import landed from being read as this content's. An import still in flight
    /// counts as the draft, since `ImportLens` has already accepted the document and
    /// answered the lens's ARN.
    pub fn published_version(&self, now: SystemTime) -> Option<(&str, &PublishedLensVersion)> {
        let content = self.summary_content(now)?;
        self.versions
            .iter()
            .filter(|(_, published)| {
                published.published_at >= content.imported_at
                    && published.content.json == content.json
            })
            // `LensVersion` is caller-chosen, so the version NAMES do not order the
            // versions; when more than one carries this content, the one published last is
            // the version the lens reports.
            .max_by(|(left_name, left), (right_name, right)| {
                left.published_at
                    .cmp(&right.published_at)
                    .then_with(|| left_name.cmp(right_name))
            })
            .map(|(version, published)| (version.as_str(), published))
    }

    /// This lens's LATEST published version, as `(LensVersion, that published version)`,
    /// or `None` while no version has been published from it.
    ///
    /// Distinct from [`Self::published_version`], which answers the version the lens's
    /// CURRENT content is published as (and nothing while that content is a draft): the
    /// latest version is the one published most recently, whatever the lens's draft now
    /// holds. `LensVersion` is caller-chosen (`CreateLensVersion`'s `LensVersion` is an
    /// input), so the version NAMES do not order the versions — the publication instant
    /// does, with the name breaking a tie between two publishes the clock cannot separate.
    pub fn latest_version(&self) -> Option<(&str, &PublishedLensVersion)> {
        self.versions
            .iter()
            .max_by(|(left_name, left), (right_name, right)| {
                left.published_at
                    .cmp(&right.published_at)
                    .then_with(|| left_name.cmp(right_name))
            })
            .map(|(version, published)| (version.as_str(), published))
    }

    /// The content a REVIEW of this lens asks: the content of its latest published
    /// version, and nothing while no version has been published from it.
    ///
    /// A custom lens "cannot be applied to workloads … until it's published with
    /// CreateLensVersion" (`ImportLens` doc) and a published version is immutable, so what
    /// a review of this lens asks is PUBLISHED content and never the draft an import left
    /// behind: re-importing a lens must not move the questions a review of it already asks.
    /// [`Self::latest_version`] is which published version that is, for the reason stated
    /// there.
    pub fn reviewed_content(&self) -> Option<&LensContent> {
        self.latest_version()
            .map(|(_, published)| &published.content)
    }

    /// The `LensType` of this lens, as the account that reaches it sees it.
    ///
    /// A custom lens is `CUSTOM_SELF` to the account that owns it and `CUSTOM_SHARED` to
    /// an account it was shared with (`smithy_query(query="shape", shape="LensType")`);
    /// every request this emulator serves is the one emulator account's
    /// ([`::engine::emulator_account_id`]), so the owner recorded on the lens is what
    /// tells the two apart.
    pub fn lens_type(&self) -> &'static str {
        if self.owner == ::engine::emulator_account_id() {
            LENS_TYPE_CUSTOM_SELF
        } else {
            LENS_TYPE_CUSTOM_SHARED
        }
    }

    /// The content a SUMMARY of this lens describes it by: its settled content when it has
    /// any, and otherwise the document of the import still in flight.
    ///
    /// A lens EXISTS from the moment `ImportLens` accepts it — the operation answers its
    /// ARN there and then — so a listing of the account's lenses reports it immediately,
    /// named by the document that import carried. The import's instant gates the lens's
    /// readable CONTENT ([`Self::settled_content`], what `ExportLens` returns), so an
    /// UPDATE to a lens that already has content keeps describing the landed content until
    /// the new import lands.
    pub fn summary_content(&self, now: SystemTime) -> Option<&LensContent> {
        self.settled_content(now)
            .or(self.pending.as_ref().map(|pending| &pending.content))
    }

    /// The `LensSummary` projection of this lens as of `now`, or `None` for a record that
    /// carries no content at all — which no import leaves behind.
    pub fn to_summary(&self, now: SystemTime) -> Option<LensSummary> {
        let content = self.summary_content(now)?;
        // The version the content this summary describes is published as, if it is: both
        // `LensVersion` and `LensStatus` speak about THAT content, so they cannot disagree.
        let published = self.published_version(now);
        Some(LensSummary {
            lens_arn: Some(self.lens_arn.clone()),
            // "For custom lenses, this is the lens ARN" (`LensAlias` doc), so the alias a
            // summary reports is the ARN every other operation resolves the lens by.
            lens_alias: Some(self.lens_arn.clone()),
            lens_name: Some(content.definition.name.clone()),
            lens_type: Some(self.lens_type().to_string()),
            description: content.definition.description.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            // "when the lens was last updated": the instant the content it is described
            // by was imported.
            updated_at: Some(::aws_smithy_types::DateTime::from(content.imported_at)),
            // "The version of the lens": the version its content was published as, and
            // [`LENS_VERSION_DRAFT`] while that content is a draft.
            lens_version: Some(
                published
                    .map_or(LENS_VERSION_DRAFT, |(version, _)| version)
                    .to_string(),
            ),
            owner: Some(self.owner.clone()),
            lens_status: Some(
                published
                    .map_or(LENS_STATUS_DRAFT, |_| LENS_STATUS_PUBLISHED)
                    .to_string(),
            ),
        })
    }
}

/// The stable enumeration key of one lens in the lens catalog `ListLenses` pages over:
/// the AWS-official lenses this emulator ships first, then the custom lenses, each group
/// in its own identifier order.
///
/// The model publishes no order for `LensSummaries`, so this is the emulator's own. What
/// it has to be is TOTAL and stable — the `NextToken` carries the key of the last lens a
/// page served, so a key that moved or repeated would serve a lens twice or skip it.
pub fn lens_catalog_key(lens_type: &str, identifier: &str) -> String {
    let rank = u8::from(lens_type != LENS_TYPE_AWS_OFFICIAL);
    format!("{rank}/{identifier}")
}

/// What an `ImportLens` idempotency token resolves to: the lens the original request
/// imported into, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct ImportLensToken {
    pub lens_id: String,
    pub fingerprint: String,
}

/// What a `CreateLensVersion` idempotency token resolves to: the lens and the version
/// the original request published, plus that request's parameter fingerprint so a reuse
/// of the token with DIFFERENT parameters can be told from a genuine replay.
#[derive(Debug, Clone)]
pub struct CreateLensVersionToken {
    pub lens_id: String,
    pub lens_version: String,
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

/// The aliases Amazon Web Services publishes its own lenses under — the AWS-OFFICIAL
/// half of the catalog a `LensAlias` resolves against.
///
/// SOURCE: the Lens Catalog of the AWS Well-Architected Tool user guide
/// (<https://docs.aws.amazon.com/wellarchitected/latest/userguide/lenses.html>), whose
/// lenses are the ones AWS's own `ListLenses` reports under `LensType$AWS_OFFICIAL`. The
/// Smithy model cannot supply the set: it bounds a `LensAlias` only by length and carries
/// `serverless` as an example of one (`smithy_query(query="shape", shape="LensAlias")`),
/// so the catalog is written out here for the same reason [`AWS_REGIONS`] is — a value
/// outside the published set names no lens at all, and an operation that APPLIES a lens
/// refuses it rather than recording it.
///
/// This emulator ships the QUESTION CONTENT of the lenses in
/// [`official_lenses::OFFICIAL_LENS_CATALOG`], whose entries are drawn from the aliases
/// below; an alias here that the content catalog does not carry is a lens this emulator
/// knows exists and holds no questions for ([`lens_questions`]), which is why this catalog
/// is a list of aliases rather than of lens definitions.
///
/// `ListLenses` therefore ENUMERATES the lenses whose content is shipped
/// ([`OfficialLens::summary`]) beside the account's custom lenses: a `LensSummary` is lens
/// CONTENT — its `LensName`, `Description`, `LensVersion` and publication instants — and
/// for an alias the content catalog does not carry this emulator holds none of that, so
/// listing it would mean inventing it. Applying one is a different question: a lens is
/// applied BY NAME, and the name is all this catalog has to resolve.
pub const AWS_OFFICIAL_LENS_ALIASES: &[&str] = &[
    "connectedmobility",
    "dataanalytics",
    "devops",
    "financialservices",
    "games",
    "genai",
    "healthcareindustry",
    "iot",
    "machinelearning",
    "mergersandacquisitions",
    "migration",
    "sap",
    "serverless",
    "softwareasaservice",
    FRAMEWORK_LENS_ALIAS,
];

/// Whether a `LensAlias` names a lens Amazon Web Services publishes: a bare alias of
/// [`AWS_OFFICIAL_LENS_ALIASES`], or an official lens ARN whose resource part ends in
/// `lens/<one of those aliases>` ([`lens_alias`], [`is_aws_official_lens`]).
pub fn is_published_official_lens(lens: &str) -> bool {
    is_aws_official_lens(lens) && AWS_OFFICIAL_LENS_ALIASES.contains(&lens_alias(lens))
}

/// What the lens a `LensAlias` names is, as of the instant it is resolved — the answer the
/// operations that APPLY a lens act on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LensResolution {
    /// A lens Amazon Web Services publishes ([`AWS_OFFICIAL_LENS_ALIASES`]). AWS's own
    /// lenses are published content, so one is applicable as it stands.
    Official,
    /// A stored custom lens whose current content a version has been published from
    /// ([`LensRecord::published_version`]) — applicable.
    PublishedCustom,
    /// A stored custom lens whose current content no version has been published from: the
    /// lens exists, and "cannot be applied to workloads … until it's published with
    /// CreateLensVersion" (`ImportLens` doc).
    DraftCustom,
    /// No lens: neither an alias of the official catalog nor a custom lens this store
    /// holds under the ARN the request named.
    Unknown,
}

/// How a `Lenses` list that names a lens which cannot be applied is refused: the modelled
/// exception for the case, for the calling operation to lift into its own error enum.
#[derive(Debug, Clone)]
pub enum LensesRefusal {
    /// An entry that names no lens at all. A lens is a named resource, so a name that
    /// resolves to none is a `ResourceNotFoundException` (HTTP 404) over the `LENS`
    /// resource type.
    NotFound(ResourceNotFoundException),
    /// An entry that names a lens which exists and cannot be applied yet. The lens
    /// resolved, so what the request got wrong is the member value — a
    /// `ValidationException` (HTTP 400) naming the member, the way `ExportLens` refuses
    /// an official lens that exists and cannot be exported.
    Unapplicable(crate::ops::types::ValidationException),
}

/// The member a lens-list refusal names (`CreateWorkloadInput$Lenses`,
/// `CreateReviewTemplateInput$Lenses`).
pub const LENSES_MEMBER: &str = "Lenses";

/// The member a lens-list refusal of an ASSOCIATION names
/// (`AssociateLensesInput$LensAliases` / `DisassociateLensesInput$LensAliases`). A refusal
/// names the member the request carried the lenses in, so the two lens-list members are
/// named apart.
pub const LENS_ALIASES_MEMBER: &str = "LensAliases";

/// The ARN of the AWS-OFFICIAL lens a `LensAlias` names, in the Region the request was
/// signed for.
///
/// "the lens ARN, such as `arn:aws:wellarchitected:us-east-1::lens/serverless`"
/// (`LensAlias` doc): an official lens ARN names no owner account, because AWS publishes
/// the lens rather than an account owning it — the same distinction
/// [`is_aws_official_lens`] reads. It is therefore the emulator's own ARN with the
/// account segment emptied, so the partition, the namespace and the account placeholder
/// still come from [`::engine::emulator_arn`] alone. A CUSTOM lens's ARN is not built
/// here: the lens that import minted owns it ([`LensRecord::lens_arn`]).
pub fn official_lens_arn(region: &str, lens: &str) -> String {
    let arn = ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("lens/{}", lens_alias(lens)));
    arn.replacen(
        &format!(":{}:", ::engine::emulator_account_id()),
        "::",
        1,
    )
}

/// The pillar priority order a workload is created with when the request names none:
/// the framework's own pillar order, which is what orders the items in a review's
/// improvement plan (`WorkloadPillarPriorities` doc: "The priorities of the pillars,
/// which are used to order items in the improvement plan").
pub fn default_pillar_priorities() -> Vec<String> {
    WELL_ARCHITECTED_PILLARS
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

/// One stored share of a workload: the principal the workload was shared WITH, the
/// permission that principal was granted, and where the share stands.
///
/// A share is its own resource, identified by its `ShareId`, and it belongs to the
/// workload it shares — `shared_by` is that workload's owning account, the principal that
/// did the sharing. It is recorded (rather than re-derived) because it is the modelled
/// `WorkloadShare$SharedBy` member, which the `WorkloadShare` shape carries and
/// `WorkloadShareSummary` does not: `ShareInvitationSummary$SharedBy` is what
/// `ListShareInvitations` reports it as, and [`WorkloadShareRecord::to_share`] — the
/// projection `UpdateWorkloadShare` answers with — reports it from this field, so the
/// principal that shared the workload survives every change to the share.
///
/// `share_invitation_id` is the id of the invitation the share's recipient has to answer.
/// The invitation is 1:1 with the share and exists from the moment the share is created
/// — "Shared access to a workload is not removed until the workload invitation is
/// deleted" (`CreateWorkloadShare` doc) — but it is a resource of its OWN, named by its
/// own `ShareInvitationId` rather than by the share's `ShareId`
/// (`smithy_query(query="shape", shape="ShareInvitationSummary")` carries both), so it
/// is minted and recorded beside it.
#[derive(Debug, Clone)]
pub struct WorkloadShareRecord {
    pub share_id: String,
    pub share_invitation_id: String,
    pub workload_id: String,
    pub shared_by: String,
    pub shared_with: String,
    pub permission_type: String,
    pub status: String,
    pub status_message: Option<String>,
}

impl WorkloadShareRecord {
    /// The `WorkloadShareSummary` projection `ListWorkloadShares` pages over.
    pub fn to_summary(&self) -> WorkloadShareSummary {
        WorkloadShareSummary {
            share_id: Some(self.share_id.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: Some(self.permission_type.clone()),
            status: Some(self.status.clone()),
            // "Optional message to compliment the Status field"
            // (`WorkloadShareSummary$StatusMessage` doc): a share that is simply waiting
            // on its invitation has nothing to add to its status.
            status_message: self.status_message.clone(),
        }
    }

    /// The `WorkloadShare` shape — "A workload share return object" — that the
    /// operations answering with the share itself return (`UpdateWorkloadShare`).
    ///
    /// It carries two members `WorkloadShareSummary` does not: `SharedBy`, the principal
    /// that did the sharing, read off the record, and `WorkloadName`, the name of the
    /// workload the share shares. The name is passed in rather than duplicated on the
    /// record because the workload owns it and `UpdateWorkload` can change it, so the
    /// share must never hold a stale copy.
    pub fn to_share(&self, workload_name: &str) -> WorkloadShare {
        WorkloadShare {
            share_id: Some(self.share_id.clone()),
            shared_by: Some(self.shared_by.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: Some(self.permission_type.clone()),
            status: Some(self.status.clone()),
            workload_name: Some(workload_name.to_string()),
            workload_id: Some(self.workload_id.clone()),
        }
    }

    /// The `ShareInvitationSummary` projection `ListShareInvitations` pages over: this
    /// share seen as the invitation its recipient was offered.
    ///
    /// `workload_name` is the name of the workload the share shares — the summary's
    /// `WorkloadName` member, which the share record does not duplicate because the
    /// workload owns it and `UpdateWorkload` can change it. Only the members of the
    /// shared resource KIND are filled: a workload invitation names a workload, and the
    /// lens, profile and review-template members of the shape belong to invitations of
    /// those kinds.
    pub fn to_invitation_summary(&self, workload_name: &str) -> ShareInvitationSummary {
        ShareInvitationSummary {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            shared_by: Some(self.shared_by.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: Some(self.permission_type.clone()),
            share_resource_type: Some(WORKLOAD_RESOURCE_TYPE.to_string()),
            workload_name: Some(workload_name.to_string()),
            workload_id: Some(self.workload_id.clone()),
            lens_name: None,
            lens_arn: None,
            profile_name: None,
            profile_arn: None,
            template_name: None,
            template_arn: None,
        }
    }

    /// The `ShareInvitation` projection `UpdateShareInvitation` answers with: the
    /// invitation this share raised, named by its own `ShareInvitationId` and by the
    /// resource it offers.
    ///
    /// Only the members of the shared resource KIND are filled — a workload invitation
    /// names a workload, and the `LensAlias`, `LensArn`, `ProfileArn` and `TemplateArn`
    /// members of the shape (`smithy_query(query="shape", shape="ShareInvitation")`)
    /// belong to invitations of those kinds.
    pub fn to_invitation(&self) -> ShareInvitation {
        ShareInvitation {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            share_resource_type: Some(WORKLOAD_RESOURCE_TYPE.to_string()),
            workload_id: Some(self.workload_id.clone()),
            lens_alias: None,
            lens_arn: None,
            profile_arn: None,
            template_arn: None,
        }
    }
}

/// What a `CreateWorkloadShare` idempotency token resolves to: the share the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateWorkloadShareToken {
    pub workload_id: String,
    pub share_id: String,
    pub fingerprint: String,
}

/// One stored share of a CUSTOM LENS: the principal the lens was shared WITH and where
/// the share stands.
///
/// The lens counterpart of [`WorkloadShareRecord`], and recorded for the same reasons: a
/// share is its own resource identified by its `ShareId`, it belongs to the lens it
/// shares, and `shared_by` is that lens's owning account — the principal that did the
/// sharing, which `ShareInvitationSummary$SharedBy` reports. A lens share carries NO
/// permission: `CreateLensShare` takes none (`smithy_query(query="operation",
/// operation="CreateLensShare")` publishes only `LensAlias`, `SharedWith` and
/// `ClientRequestToken`), so a lens invitation reports none either.
///
/// `share_invitation_id` is the id of the invitation the recipient has to answer. It is
/// 1:1 with the share and exists from the moment the share is created — "Shared access
/// to a lens is not removed until the lens invitation is deleted" (`CreateLensShare`
/// doc) — but it is a resource of its OWN, named by its own `ShareInvitationId`, so it
/// is minted and recorded beside the `ShareId`.
///
/// `lens_arn` is held by value because the invitation reports it
/// (`ShareInvitationSummary$LensArn`) and a lens's ARN never changes; the lens's NAME is
/// lens CONTENT, which a later `ImportLens` can change, so it is read from the lens
/// record at projection time instead.
#[derive(Debug, Clone)]
pub struct LensShareRecord {
    pub share_id: String,
    pub share_invitation_id: String,
    /// The lens id the share belongs to — the key of [`LensRecord`] in the store.
    pub lens_id: String,
    pub lens_arn: String,
    pub shared_by: String,
    pub shared_with: String,
    pub status: String,
    pub status_message: Option<String>,
}

impl LensShareRecord {
    /// The `ShareInvitationSummary` projection `ListShareInvitations` pages over: this
    /// share seen as the invitation its recipient was offered.
    ///
    /// `lens_name` is the name of the lens the share shares, which the lens's CONTENT
    /// owns, so the caller reads it off the lens record rather than off the share; it is
    /// `None` for a lens that carries no readable content. Only the members of the shared
    /// resource KIND are filled, and a lens share grants no permission, so
    /// `PermissionType` is left off too.
    pub fn to_invitation_summary(&self, lens_name: Option<&str>) -> ShareInvitationSummary {
        ShareInvitationSummary {
            share_invitation_id: Some(self.share_invitation_id.clone()),
            shared_by: Some(self.shared_by.clone()),
            shared_with: Some(self.shared_with.clone()),
            permission_type: None,
            share_resource_type: Some(LENS_RESOURCE_TYPE.to_string()),
            workload_name: None,
            workload_id: None,
            lens_name: lens_name.map(ToString::to_string),
            lens_arn: Some(self.lens_arn.clone()),
            profile_name: None,
            profile_arn: None,
            template_name: None,
            template_arn: None,
        }
    }
}

/// What a `CreateLensShare` idempotency token resolves to: the share the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateLensShareToken {
    pub lens_id: String,
    pub share_id: String,
    pub fingerprint: String,
}

/// What a `CreateReviewTemplate` idempotency token resolves to: the review template the
/// original request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateReviewTemplateToken {
    pub template_arn: String,
    pub fingerprint: String,
}

/// The `ProfileVersion` a profile carries the moment it is created.
///
/// A profile is VERSIONED — `GetProfile` reads one version of it
/// (`GetProfileInput$ProfileVersion`), `UpgradeProfileVersion` moves a workload from the
/// version it holds to a later one, and `ProfileNotificationSummary` compares a
/// workload's `CurrentProfileVersion` with the profile's `LatestProfileVersion`
/// (`smithy_query(query="shape", shape="ProfileNotificationSummary")`) — so the version
/// is the SERVICE's own counter over a profile's revisions, not a value a request
/// carries. A profile nothing has revised yet is at its first revision, and `1`
/// satisfies the modelled `ProfileVersion` (`@length(1, 32)`, `^[A-Za-z0-9-]+$`).
/// Minting the next one belongs to `UpdateProfile`, still a stub with its own issue.
pub const INITIAL_PROFILE_VERSION: &str = "1";

/// The most Well-Architected Tool profiles this emulator holds for its one account and
/// Region.
///
/// `CreateProfile` models `ServiceQuotaExceededException` (HTTP 402, "The user has
/// reached their resource quota") for an account that has reached its profile quota; the
/// real quota is adjustable through Service Quotas, so the number here is this emulator's
/// own ceiling rather than one read off a doc page. It is what bounds
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::profiles`] (and,
/// with it, the create ledger that is 1:1 with a live profile), and it sits far above any
/// realistic client workflow, so a caller never meets it by accident.
pub const MAX_PROFILES: usize = 1_000;

/// The most profile questions one profile records here.
///
/// In the real service the questions come from the ONE profile template the service
/// publishes (`GetProfileTemplate`), so their number is fixed by that template rather
/// than by the request. This emulator carries no such template — `GetProfileTemplate` is
/// a stub with its own issue — so this is the ceiling that keeps a stored profile's
/// question map bounded, answered with the modelled `ServiceQuotaExceededException` for
/// the reason [`MAX_PROFILES`] gives.
pub const MAX_QUESTIONS_PER_PROFILE: usize = 100;

/// The modelled input member a profile-question finding names
/// (`CreateProfileInput$ProfileQuestions` / `UpdateProfileInput$ProfileQuestions`).
pub const PROFILE_QUESTIONS_MEMBER: &str = "ProfileQuestions";

/// One stored Well-Architected Tool profile: the resource `CreateProfile` mints and
/// `GetProfile` reports.
///
/// The record holds what the request carried plus what the SERVICE owns — the ARN it
/// minted, the version it is at, the account it attributed the create to and when it
/// happened. The `Profile` shape is DERIVED from it by [`ProfileRecord::to_profile`], the
/// only place a response is built from this record, so a read cannot drift from what is
/// stored.
#[derive(Debug, Clone)]
pub struct ProfileRecord {
    pub profile_arn: String,
    pub profile_version: String,
    pub profile_name: String,
    pub profile_description: String,
    /// The profile's answered questions: `QuestionId` -> the choices selected for it.
    ///
    /// A `BTreeMap` because a profile records ONE answer per question — the modelled
    /// `Profile$ProfileQuestions` reports each question once — and question-id order is
    /// then the stable order every read reports them in. The selection is a `BTreeSet`
    /// for the same reason: "The selected choices"
    /// (`ProfileQuestionUpdate$SelectedChoiceIds`) is the set of choices in force, so a
    /// choice is either selected or not and naming one twice selects it once.
    pub questions: BTreeMap<String, BTreeSet<String>>,
    pub owner: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The profile's tags, by key — a `BTreeMap` so they are reported in one stable
    /// order, as a workload's and a review template's tags are held.
    pub tags: BTreeMap<String, String>,
}

impl ProfileRecord {
    /// The `Profile` shape `GetProfile` returns.
    pub fn to_profile(&self) -> Profile {
        Profile {
            profile_arn: Some(self.profile_arn.clone()),
            profile_version: Some(self.profile_version.clone()),
            profile_name: Some(self.profile_name.clone()),
            profile_description: Some(self.profile_description.clone()),
            profile_questions: Some(self.profile_questions()),
            owner: Some(self.owner.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            // "The ID assigned to the share invitation": only a profile reached through a
            // share invitation carries one, and profile shares are still stubs with their
            // own issues, so a profile this account owns carries none.
            share_invitation_id: None,
            // The tag map the profile carries, empty when it carries none: the
            // `@length(min: 1)` on `TagMap` constrains the tags a REQUEST supplies, not
            // what a projection of the stored profile reports.
            tags: Some(self.tags.clone().into_iter().collect()),
        }
    }

    /// The stored questions as the modelled `Profile$ProfileQuestions` carries them.
    ///
    /// `QuestionTitle`, `QuestionDescription`, `QuestionChoices`, `MinSelectedChoices`
    /// and `MaxSelectedChoices` describe the question itself, and the service reads them
    /// off the profile TEMPLATE it publishes (the same members on
    /// `ProfileTemplateQuestion` — `smithy_query(query="shape",
    /// shape="ProfileTemplateQuestion")`). This emulator carries no profile template —
    /// `GetProfileTemplate` is a stub with its own issue — so they are reported ABSENT
    /// rather than invented, and what is reported is exactly what the profile records:
    /// the question and the choices selected for it.
    pub fn profile_questions(&self) -> Vec<ProfileQuestion> {
        self.questions
            .iter()
            .map(|(question_id, selected)| ProfileQuestion {
                question_id: Some(question_id.clone()),
                selected_choice_ids: Some(selected.iter().cloned().collect()),
                question_title: None,
                question_description: None,
                question_choices: None,
                min_selected_choices: None,
                max_selected_choices: None,
            })
            .collect()
    }

    /// The questions as [`parameter_fingerprint`] input: the NORMALIZED question map, so
    /// two `CreateProfile` requests fingerprint alike exactly when they would record the
    /// same selections — the order the questions arrive in, and a choice named twice, do
    /// not make a different request.
    pub fn question_fingerprint(&self) -> ::serde_json::Value {
        ::serde_json::json!(self
            .questions
            .iter()
            .map(|(question_id, selected)| (
                question_id.clone(),
                selected.iter().cloned().collect::<Vec<String>>()
            ))
            .collect::<BTreeMap<String, Vec<String>>>())
    }
}

/// What a `CreateProfile` idempotency token resolves to: the profile the original request
/// created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateProfileToken {
    pub profile_arn: String,
    pub fingerprint: String,
}

/// What an `UpgradeProfileVersion` idempotency token resolves to: the profile whose
/// association the original request upgraded, plus that request's parameter fingerprint.
///
/// The operation answers an EMPTY body, so a replay has no value to return — what the
/// token is for here is keeping the replay from acting a second time, which matters
/// because the upgrade can also record a milestone and a second recording of the same
/// milestone name would collide ([`milestone_name_conflict`]). The profile ARN is kept so
/// a token reused with different parameters can name the resource it already upgraded, and
/// the workload id so the token goes with the workload whose association it upgraded
/// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::remove_workload`]).
#[derive(Debug, Clone)]
pub struct UpgradeProfileVersionToken {
    pub workload_id: String,
    pub profile_arn: String,
    pub fingerprint: String,
}

/// `ConflictException$ResourceType` / `ResourceNotFoundException$ResourceType` for an
/// AGENT profile — the resource `CreateAgentProfile` mints and every
/// `/api/v1/agent-profiles/{profileArn}` route resolves.
///
/// Deliberately NOT [`PROFILE_RESOURCE_TYPE`]: the Well-Architected Tool profile of
/// `CreateProfile` is a different resource, named by a different ARN (`ProfileArn`'s
/// `profile/…` against `AgentProfileArn`'s `agent-profile/…` —
/// `smithy_query(query="shape", shape="AgentProfileArn")`), so a failure about one must
/// not claim to be about the other.
pub const AGENT_PROFILE_RESOURCE_TYPE: &str = "AGENT_PROFILE";

/// `ResourceNotFoundException$ResourceType` for an optimization goal of an agent
/// profile. `ExceptionResourceType` is a free-form string
/// (`smithy_query(query="shape", shape="ExceptionResourceType")`), so it names the kind
/// of resource that was missing: a goal.
pub const AGENT_GOAL_RESOURCE_TYPE: &str = "GOAL";

/// `ResourceNotFoundException$ResourceType` / `ConflictException$ResourceType` for a
/// context of an agent profile — "application and environment information used during
/// recommendation generation" (`CreateAgentContext` doc). `ExceptionResourceType` is a
/// free-form string, so it names the kind of resource the failure is about: a context.
pub const AGENT_CONTEXT_RESOURCE_TYPE: &str = "CONTEXT";

/// The most agent profiles this emulator holds.
///
/// `CreateAgentProfile` models `ServiceQuotaExceededException` (HTTP 402, "The user has
/// reached their resource quota") and the real quota is adjustable through Service
/// Quotas, so the number here is this emulator's own ceiling rather than a value read
/// off a doc page — it is what bounds [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::agent_profiles`]
/// (and, with it, the create ledger that is 1:1 with a live profile), and it sits far
/// above any realistic client workflow.
pub const MAX_AGENT_PROFILES: usize = 1_000;

/// The most optimization goals one agent profile holds here. `CreateAgentGoal` models
/// `ServiceQuotaExceededException` for the case; the ceiling is this emulator's own, for
/// the reasons [`MAX_AGENT_PROFILES`] gives, and it bounds the goal map per profile.
pub const MAX_GOALS_PER_AGENT_PROFILE: usize = 100;

/// The most contexts one agent profile holds here. `CreateAgentContext` models
/// `ServiceQuotaExceededException` for the case; the ceiling is this emulator's own, for
/// the reasons [`MAX_AGENT_PROFILES`] gives, and it bounds the context map per profile.
pub const MAX_CONTEXTS_PER_AGENT_PROFILE: usize = 100;

/// The largest page the agent-profile list operations serve, and the ceiling their
/// `maxResults` is held to.
///
/// Unlike the workload-era lists ([`MAX_RESULTS_CEILING`]), every agent list member is
/// modelled `@range(min: 1, max: 1000)` with `@default(100)` —
/// `smithy_query(query="operation", operation="ListAgentContexts")` shows
/// `maxResults` that way — so the agent lists get their own ceiling rather than the
/// workload one.
pub const MAX_AGENT_LIST_RESULTS: i32 = 1_000;

/// The page size an agent list request asks for: the modelled
/// `@range(min: 1, max: 1000)` restated in the handler, so the page size is in
/// `1..=MAX_AGENT_LIST_RESULTS` whatever reaches it. The member carries
/// `@default(100)`, so a request that omits it arrives here already carrying 100.
pub fn agent_list_page_size(max_results: i32) -> usize {
    let clamped = max_results.clamp(1, MAX_AGENT_LIST_RESULTS);
    usize::try_from(clamped).expect(
        "maxResults is clamped into 1..=MAX_AGENT_LIST_RESULTS, which always fits in a usize",
    )
}

/// The modelled input member a pillar-set finding names
/// (`CreateAgentProfileRequest$pillars`, `CreateAgentGoalRequest$pillars`), as
/// `ValidationExceptionField$Name` reports it.
pub const PILLARS_MEMBER: &str = "pillars";

/// The refusal for a `Pillars` list that names a value the `Pillar` enum does not
/// publish, or `None` when every entry is published.
///
/// `Pillar` is an enum, so the boundary walk in `crate::service` refuses such a value with
/// this same 400 before a handler runs; every operation that STORES a pillar list keeps
/// the check as defence in depth for a caller that builds the input directly, so the
/// sentence they all refuse it with is built here rather than once per operation.
pub fn unpublished_pillar_exception(
    pillars: &[String],
) -> Option<crate::ops::types::ValidationException> {
    let published = crate::ops::types::PILLAR_VALUES;
    let unknown = pillars
        .iter()
        .find(|pillar| !published.contains(&pillar.as_str()))?;
    Some(member_constraint_exception(
        PILLARS_MEMBER,
        &format!(
            "Member must satisfy enum value set: [{}], got {unknown}",
            published.join(", ")
        ),
    ))
}

/// The modelled input member a context-type finding names
/// (`CreateAgentContextRequest$contextType`), as `ValidationExceptionField$Name` reports
/// it.
pub const CONTEXT_TYPE_MEMBER: &str = "contextType";

/// The member an application-type finding of a context's content names
/// (`ContextContent$applicationType`, reached through `…Request$content`), as the path
/// `ValidationExceptionField$Name` reports for a member inside a structure.
pub const CONTENT_APPLICATION_TYPE_MEMBER: &str = "content.applicationType";

/// The member a criticality finding of a context's content names
/// (`ContextContent$criticality`). See [`CONTENT_APPLICATION_TYPE_MEMBER`].
pub const CONTENT_CRITICALITY_MEMBER: &str = "content.criticality";

/// The refusal for a context whose enum-typed members carry a value their enum does not
/// publish, or `None` when every one of them does.
///
/// Defence in depth for a caller that builds the input directly: `ContextType`,
/// `ApplicationType` and `Criticality` are enums, so the boundary walk in
/// `crate::service` already refuses a value outside their published sets with this same
/// 400 before a handler runs. It is kept so a context is never STORED naming a value the
/// enums do not publish, and shared by every operation that writes one so they cannot
/// word the refusal differently.
pub fn unknown_context_enum_exception(
    context_type: Option<&str>,
    content: &ContextContent,
) -> Option<crate::ops::types::ValidationException> {
    let candidates: [(&str, Option<&str>, &[&str]); 3] = [
        (CONTEXT_TYPE_MEMBER, context_type, CONTEXT_TYPE_VALUES),
        (
            CONTENT_APPLICATION_TYPE_MEMBER,
            content.application_type.as_deref(),
            APPLICATION_TYPE_VALUES,
        ),
        (
            CONTENT_CRITICALITY_MEMBER,
            content.criticality.as_deref(),
            CRITICALITY_VALUES,
        ),
    ];
    let findings: Vec<(String, String)> = candidates
        .iter()
        .filter_map(|(member, value, values)| {
            let value = (*value)?;
            (!values.contains(&value)).then(|| {
                (
                    (*member).to_string(),
                    constraint_sentence(
                        member,
                        &format!(
                            "Member must satisfy enum value set: [{}], got {value}",
                            values.join(", ")
                        ),
                    ),
                )
            })
        })
        .collect();
    (!findings.is_empty()).then(|| field_validation_exception(&findings))
}

/// The modelled input member an agent profile's generation-eligibility finding names
/// (`CreateAgentProfileRequest$aggregationConfiguration`), as
/// `CreateAgentProfileResponse$fieldErrors` keys it — "a map of field paths to error
/// messages for invalid or missing input fields".
pub const AGGREGATION_CONFIGURATION_MEMBER: &str = "aggregationConfiguration";

/// What `fieldErrors` says about a profile that names no scope to analyse, and so is
/// not eligible for recommendation generation (see
/// [`AgentProfileRecord::eligible_for_generation`]).
pub const NO_AGGREGATION_CONFIGURATION_MESSAGE: &str =
    "At least one aggregation configuration is required to generate recommendations.";

/// One stored agent profile: the resource `CreateAgentProfile` mints, `GetAgentProfile`
/// reports and the goals below belong to.
///
/// The record holds what the request carried plus what the SERVICE owns — the ARN it
/// minted, the identity it attributed the create to and when it happened — and the
/// reporting members (`eligibleFor…`, `fieldErrors`) are DERIVED from it rather than
/// stored, so they cannot drift from the profile they describe.
#[derive(Debug, Clone)]
pub struct AgentProfileRecord {
    pub arn: String,
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub business_overview: Option<String>,
    pub pillars: Vec<String>,
    pub deletion_protection: bool,
    pub execution_role_arn: String,
    pub aggregation_configuration: Vec<AggregationConfiguration>,
    /// The profile's tags, by key — a `BTreeMap` so they are reported in one stable
    /// order, as a workload's and a review template's tags are held.
    pub tags: BTreeMap<String, String>,
    pub created_by: String,
    pub created_at: SystemTime,
    pub last_modified_by: Option<String>,
    pub last_modified_at: Option<SystemTime>,
}

impl AgentProfileRecord {
    /// Whether this profile is "valid for scheduled recommendation generation" /
    /// "valid for manual architecture generation"
    /// (`CreateAgentProfileResponse$eligibleForScheduledGeneration` /
    /// `$eligibleForArchitectureGeneration`).
    ///
    /// Generation analyses the Amazon Web Services resources the profile's
    /// `aggregationConfiguration` scopes it to ("configuration settings that define the
    /// scope of Amazon Web Services resources to analyze for optimization
    /// recommendations" — `smithy_query(query="shape",
    /// shape="AggregationConfiguration")`), and the list is modelled `@length(min: 0)`,
    /// so a profile that names no account and Region has nothing to analyse and is
    /// eligible for neither. Both eligibility members answer the same question of the
    /// same stored scope, so they are derived from it together.
    pub fn eligible_for_generation(&self) -> bool {
        !self.aggregation_configuration.is_empty()
    }

    /// `fieldErrors`: one entry per input member that leaves this profile unable to
    /// generate recommendations, or `None` when none does. Derived from the stored
    /// profile, so it reports what is actually wrong with it rather than what one
    /// request looked like.
    pub fn field_errors(&self) -> Option<HashMap<String, String>> {
        (!self.eligible_for_generation()).then(|| {
            HashMap::from([(
                AGGREGATION_CONFIGURATION_MEMBER.to_string(),
                NO_AGGREGATION_CONFIGURATION_MESSAGE.to_string(),
            )])
        })
    }

    /// The `AgentProfileSummary` projection the profile LIST reports
    /// (`ListAgentProfilesResponse$items`), so one place decides how a stored profile is
    /// summarised.
    ///
    /// The same members `CreateAgentProfile` and `GetAgentProfile` answer from the same
    /// record — the model declares `AgentProfileSummary` as its own structure
    /// (`smithy_query(query="shape", shape="AgentProfileSummary")`), so this is a third
    /// Rust type over the one stored record — with the reporting members
    /// (`eligibleFor…`, `fieldErrors`, `tags`) DERIVED from it rather than held twice, so
    /// a listed profile cannot disagree with the one a read of it reports.
    pub fn to_summary(&self) -> AgentProfileSummary {
        AgentProfileSummary {
            name: self.name.clone(),
            display_name: self.display_name.clone(),
            description: self.description.clone(),
            business_overview: self.business_overview.clone(),
            pillars: self.pillars.clone(),
            deletion_protection: self.deletion_protection,
            execution_role_arn: self.execution_role_arn.clone(),
            aggregation_configuration: self.aggregation_configuration.clone(),
            arn: self.arn.clone(),
            eligible_for_scheduled_generation: Some(self.eligible_for_generation()),
            eligible_for_architecture_generation: Some(self.eligible_for_generation()),
            field_errors: self.field_errors(),
            tags: self.tag_list(),
            created_by: self.created_by.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_by: self.last_modified_by.clone(),
            last_modified_at: self
                .last_modified_at
                .map(::aws_smithy_types::DateTime::from),
        }
    }

    /// The profile's tags as the wire carries them, or `None` when it has none: the
    /// modelled `Tags` member is a list of `Tag` key/value pairs
    /// (`smithy_query(query="shape", shape="Tag")`).
    pub fn tag_list(&self) -> Option<Vec<Tag>> {
        (!self.tags.is_empty()).then(|| {
            self.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect()
        })
    }
}

/// One stored optimization goal of an agent profile: "specific targets and objectives
/// for the optimization process" (`CreateAgentGoal` doc).
///
/// A goal belongs to the profile its `profileArn` names — it is reached only through
/// that profile's `/goals` routes — and carries, beside what the request sent, the id
/// the service minted for it and the identity and instant the create is attributed to.
#[derive(Debug, Clone)]
pub struct AgentGoalRecord {
    pub id: String,
    pub profile_arn: String,
    pub pillars: Vec<String>,
    pub title: String,
    pub description: Option<String>,
    pub created_by: String,
    pub created_at: SystemTime,
    pub last_modified_by: Option<String>,
    pub last_modified_at: Option<SystemTime>,
}

impl AgentGoalRecord {
    /// The `GoalSummary` projection every goal operation answers with
    /// (`CreateAgentGoal`, `GetAgentGoal`, `ListAgentGoals` and `UpdateAgentGoal`), so
    /// one place decides how a stored goal is reported.
    pub fn to_summary(&self) -> GoalSummary {
        GoalSummary {
            id: self.id.clone(),
            profile_arn: self.profile_arn.clone(),
            pillars: self.pillars.clone(),
            title: self.title.clone(),
            description: self.description.clone(),
            created_by: self.created_by.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_by: self.last_modified_by.clone(),
            last_modified_at: self
                .last_modified_at
                .map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// One stored context of an agent profile: "application and environment information used
/// during recommendation generation" (`CreateAgentContext` doc).
///
/// A context belongs to the profile its `profileArn` names — it is reached only through
/// that profile's `/contexts` routes — and carries, beside what the request sent, the id
/// the service minted for it and the identity and instant the create is attributed to.
/// Held exactly as a goal is, for the same reason: several operations reach it.
#[derive(Debug, Clone)]
pub struct AgentContextRecord {
    pub id: String,
    pub profile_arn: String,
    pub title: String,
    pub context_type: String,
    pub content: ContextContent,
    pub created_by: String,
    pub created_at: SystemTime,
    pub last_modified_by: Option<String>,
    pub last_modified_at: Option<SystemTime>,
}

impl AgentContextRecord {
    /// The `ContextSummary` projection every context operation answers with
    /// (`CreateAgentContext`, `ListAgentContexts`, and — once they are implemented —
    /// `GetAgentContext` and `UpdateAgentContext`), so one place decides how a stored
    /// context is reported.
    ///
    /// `applicationType` and `criticality` are DERIVED from the stored content rather
    /// than held twice: the summary's members describe "the application described by
    /// this context" and `ContextContent` is where that application's type and
    /// criticality are recorded (`smithy_query(query="shape",
    /// shape="ContextSummary")` / `shape="ContextContent"`), so they cannot drift from
    /// the content they summarise.
    pub fn to_summary(&self) -> ContextSummary {
        ContextSummary {
            id: self.id.clone(),
            profile_arn: self.profile_arn.clone(),
            title: self.title.clone(),
            context_type: self.context_type.clone(),
            application_type: self.content.application_type.clone(),
            criticality: self.content.criticality.clone(),
            content: self.content.clone(),
            created_by: self.created_by.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            last_modified_by: self.last_modified_by.clone(),
            last_modified_at: self
                .last_modified_at
                .map(::aws_smithy_types::DateTime::from),
        }
    }
}

/// `ResourceNotFoundException$ResourceType` for a recommendation generation of an agent
/// profile — the job `StartAgentRecommendationGeneration` accepts and
/// `GetAgentRecommendationGeneration` reports. `ExceptionResourceType` is a free-form
/// string, so it names the kind of resource that was missing: a generation.
pub const AGENT_GENERATION_RESOURCE_TYPE: &str = "GENERATION";

/// The most recommendation generations one agent profile holds here.
/// `StartAgentRecommendationGeneration` models `ServiceQuotaExceededException` (HTTP 402)
/// for the case; the ceiling is this emulator's own, for the reasons
/// [`MAX_AGENT_PROFILES`] gives, and it is what bounds
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::agent_generations`]
/// — nothing deletes a generation, so the map needs a bound of its own.
pub const MAX_GENERATIONS_PER_AGENT_PROFILE: usize = 100;

/// `GenerationStatus$QUEUED`: a generation this service has accepted and not started
/// working on yet (`smithy_query(query="shape", shape="GenerationStatus")`).
pub const GENERATION_QUEUED: &str = "QUEUED";

/// `GenerationStatus$IN_PROGRESS`: a generation whose analysis has started and has not
/// finished.
pub const GENERATION_IN_PROGRESS: &str = "IN_PROGRESS";

/// `GenerationStatus$COMPLETED`: the terminal status of a generation that finished —
/// "poll this operation until status reaches a terminal value of COMPLETED (results are
/// ready) or ERROR" (`GetAgentRecommendationGeneration` doc).
///
/// The enum's fourth member, `ERROR`, is the terminal status of a generation that FAILED,
/// whose `errorDetails` says why. No generation this emulator accepts reaches it — a
/// start it cannot serve is refused by `StartAgentRecommendationGeneration` instead — so
/// it is not named as a constant here; `GENERATION_STATUS_VALUES` carries the whole
/// published catalog.
pub const GENERATION_COMPLETED: &str = "COMPLETED";

/// How long after `StartAgentRecommendationGeneration` accepts a generation its analysis
/// STARTS — the span the generation spends `QUEUED`.
///
/// AWS publishes no duration for either span, so both are this emulator's own: non-zero
/// on purpose, so a caller polling straight after the start call still observes `QUEUED`,
/// and short enough not to hold a workflow up. Nothing sleeps for it —
/// [`AgentGenerationRecord::status`] compares the stored instants with the clock on every
/// read.
pub const GENERATION_QUEUE_DURATION: Duration = Duration::from_millis(250);

/// How long a generation's analysis takes once it has started — the span the generation
/// spends `IN_PROGRESS` before it reaches `COMPLETED`. This emulator's own span, for the
/// reasons [`GENERATION_QUEUE_DURATION`] gives.
pub const GENERATION_RUN_DURATION: Duration = Duration::from_millis(500);

/// One accepted recommendation generation: the asynchronous job
/// `StartAgentRecommendationGeneration` records and `GetAgentRecommendationGeneration`
/// reports the status and progress of.
///
/// The record holds what the request carried plus what the SERVICE owns — the id it
/// minted, the identity and instant it attributed the start to, and the two instants the
/// clock owes the job's transitions at. Status, progress and the transition timestamps
/// are DERIVED from those instants rather than stored, so they cannot drift from the
/// clock and no read has to write.
#[derive(Debug, Clone)]
pub struct AgentGenerationRecord {
    pub id: String,
    pub profile_arn: String,
    pub name: Option<String>,
    /// `StartAgentRecommendationGenerationRequest$types` — the kinds of recommendation
    /// this generation was started for. Stored because it is what a listing's
    /// `recommendationType` filter selects on and what the recommendations a generation
    /// produces are of; neither response shape projects it
    /// (`smithy_query(query="operation", operation="StartAgentRecommendationGeneration")`),
    /// and `ListAgentRecommendationGenerations` — the operation that filters on it — is
    /// still a stub with its own issue.
    pub types: Vec<String>,
    pub scope: Scope,
    pub additional_context: Option<::aws_smithy_types::Document>,
    pub created_by: String,
    pub created_at: SystemTime,
    /// When the analysis starts: `created_at` plus [`GENERATION_QUEUE_DURATION`].
    pub starts_at: SystemTime,
    /// When the analysis finishes: `starts_at` plus [`GENERATION_RUN_DURATION`].
    pub ends_at: SystemTime,
    /// The fingerprint of the request that started it, so a repeat of that request —
    /// `StartAgentRecommendationGeneration` carries `@idempotent` — resolves to this
    /// generation instead of starting a second one.
    pub fingerprint: String,
}

impl AgentGenerationRecord {
    /// The `GenerationStatus` of this generation as of `now`: `QUEUED` until its analysis
    /// starts, `IN_PROGRESS` until it finishes, then the terminal `COMPLETED`.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now < self.starts_at {
            GENERATION_QUEUED
        } else if now < self.ends_at {
            GENERATION_IN_PROGRESS
        } else {
            GENERATION_COMPLETED
        }
    }

    /// Whether this generation has reached a terminal status as of `now` — what decides
    /// whether a repeat of the request that started it resolves to it.
    pub fn is_terminal(&self, now: SystemTime) -> bool {
        self.status(now) == GENERATION_COMPLETED
    }

    /// `startedAt` — "the timestamp when the recommendation generation process started",
    /// absent while the generation is still `QUEUED`: nothing has started yet.
    pub fn started_at(&self, now: SystemTime) -> Option<SystemTime> {
        (self.starts_at <= now).then_some(self.starts_at)
    }

    /// `endedAt` — "the timestamp when the recommendation generation process completed",
    /// absent until it has.
    pub fn ended_at(&self, now: SystemTime) -> Option<SystemTime> {
        (self.ends_at <= now).then_some(self.ends_at)
    }

    /// `lastModifiedAt` / `lastModifiedBy`: the instant of the most recent transition the
    /// clock has made due, and the identity that made it.
    ///
    /// A generation's stored state is changed by the SERVICE moving its status, so the
    /// most recent change is the latest transition that has happened; while the generation
    /// is still `QUEUED` nothing has changed since the start call, so both members are
    /// absent rather than echoing it — the convention every create in this service
    /// follows.
    pub fn last_modified(&self, now: SystemTime) -> Option<(String, SystemTime)> {
        self.ended_at(now)
            .or_else(|| self.started_at(now))
            .map(|at| (::engine::emulator_account_id().to_string(), at))
    }

    /// `progress` — "current progress information including steps completed and completion
    /// percentage", as of `now`.
    ///
    /// A generation analyses the pillars its `scope` names ("the Well-Architected Tool
    /// Framework pillars to include in the generation scope" —
    /// `smithy_query(query="shape", shape="Scope")`), so one scoped pillar is one step of
    /// the process and `totalSteps` is how many the scope names. `stepsCompleted` is how
    /// many of them the elapsed share of [`GENERATION_RUN_DURATION`] has got through, and
    /// `completionPercentage` is that same share of the steps as a percentage, so the two
    /// members always agree.
    pub fn progress(&self, now: SystemTime) -> Progress {
        let total = i32::try_from(self.scope.pillars.len()).unwrap_or(i32::MAX);
        let completed = if now >= self.ends_at {
            total
        } else if now <= self.starts_at || total <= 0 {
            0
        } else {
            let elapsed = now
                .duration_since(self.starts_at)
                .unwrap_or_default()
                .as_secs_f64();
            let share = (elapsed / GENERATION_RUN_DURATION.as_secs_f64()).clamp(0.0, 1.0);
            // `share < 1.0` here, so the floor is at most `total - 1`: a generation that
            // has not ended never reports every step done.
            (share * f64::from(total)) as i32
        };
        Progress {
            steps_completed: completed,
            total_steps: total,
            completion_percentage: if total <= 0 {
                0.0
            } else {
                f64::from(completed) * 100.0 / f64::from(total)
            },
        }
    }
}

/// What a `CreateAgentProfile` idempotency token resolves to: the profile the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateAgentProfileToken {
    pub profile_arn: String,
    pub fingerprint: String,
}

/// What an `UpdateAgentProfile` idempotency token resolves to: the profile the original
/// request updated, plus that request's parameter fingerprint, so a retry carrying the
/// token can be told from a later request that happens to reuse it for other values.
#[derive(Debug, Clone)]
pub struct UpdateAgentProfileToken {
    pub profile_arn: String,
    pub fingerprint: String,
}

/// What a `CreateAgentGoal` idempotency token resolves to: the goal the original request
/// created, the profile it belongs to, and that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateAgentGoalToken {
    pub profile_arn: String,
    pub goal_id: String,
    pub fingerprint: String,
}

/// What an `UpdateAgentGoal` idempotency token resolves to: the goal the original request
/// updated, the profile it belongs to, and that request's parameter fingerprint.
///
/// The update itself has no value to mint, so what the token is for here is keeping a
/// retry from acting a SECOND time: the replay answers the goal as the original request
/// left it instead of stamping `lastModifiedAt` again.
#[derive(Debug, Clone)]
pub struct UpdateAgentGoalToken {
    pub profile_arn: String,
    pub goal_id: String,
    pub fingerprint: String,
}

/// What a `CreateAgentContext` idempotency token resolves to: the context the original
/// request created, the profile it belongs to, and that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateAgentContextToken {
    pub profile_arn: String,
    pub context_id: String,
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
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_milestone_tokens: TokenLedger<CreateMilestoneToken>,
    /// Every share recorded against a workload, keyed by `WorkloadId` then by `ShareId`.
    /// A `BTreeMap` because `ShareId` order is the stable order `ListWorkloadShares`
    /// pages over.
    pub workload_shares: HashMap<String, BTreeMap<String, WorkloadShareRecord>>,
    /// `CreateWorkloadShare`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_workload_share_tokens: TokenLedger<CreateWorkloadShareToken>,
    /// Every share recorded against a custom lens, keyed by the lens id then by
    /// `ShareId`. A `BTreeMap` because `ShareId` order is the stable order a listing of
    /// a lens's shares pages over, exactly as [`Self::workload_shares`] is ordered.
    pub lens_shares: HashMap<String, BTreeMap<String, LensShareRecord>>,
    /// `CreateLensShare`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_lens_share_tokens: TokenLedger<CreateLensShareToken>,
    /// Normalized workload name -> `WorkloadId`. Backs the documented "name must be
    /// unique within an account within a Region; spaces and capitalization are
    /// ignored" rule.
    pub workload_name_index: HashMap<String, String>,
    /// `CreateWorkload`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_workload_tokens: TokenLedger<CreateWorkloadToken>,
    /// `DeleteWorkload`'s `ClientRequestToken` -> the delete the original request
    /// completed ([`delete_replay`]), so a retry carrying it replays that success instead
    /// of resolving an id the original request removed.
    ///
    /// It deliberately OUTLIVES the workload it names — that is the whole point of it,
    /// and why [`Self::remove_workload`] (which drops the ledgers that resolve to a live
    /// workload) leaves it alone. Nothing a live resource bounds it, so the
    /// [`TokenLedger`]'s own ceiling ([`MAX_REMEMBERED_REQUEST_TOKENS`]) is what does:
    /// write it through [`TokenLedger::remember`].
    pub delete_workload_tokens: TokenLedger<CompletedDeleteToken>,
    /// Every stored review template, keyed by its `TemplateArn` — the only identifier
    /// the modelled operations take for one (`GetReviewTemplate`,
    /// `UpdateReviewTemplate` and `DeleteReviewTemplate` all bind `TemplateArn` as the
    /// URI label).
    pub review_templates: HashMap<String, ReviewTemplateRecord>,
    /// `CreateReviewTemplate`'s `ClientRequestToken` -> what the original request
    /// created. Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_review_template_tokens: TokenLedger<CreateReviewTemplateToken>,
    /// `DeleteReviewTemplate`'s `ClientRequestToken` -> the delete the original request
    /// completed, held exactly as [`Self::delete_workload_tokens`] is and for the same
    /// reason: the deleted template cannot serve the replay, so the completed delete does.
    pub delete_review_template_tokens: TokenLedger<CompletedDeleteToken>,
    /// `UpgradeReviewTemplateLensReview`'s `ClientRequestToken` -> the upgrade the
    /// original request completed, held exactly as [`Self::delete_review_template_tokens`]
    /// is and for the same reason: an upgrade answers an empty success, so the completed
    /// upgrade is what a retry is served from ([`CompletedUpgradeToken`]).
    pub upgrade_review_template_lens_review_tokens: TokenLedger<CompletedUpgradeToken>,
    /// Every stored Well-Architected Tool profile, keyed by its `ProfileArn` — the only
    /// identifier the modelled profile operations take for one (`GetProfile`,
    /// `UpdateProfile`, `DeleteProfile` and every `/profiles/{ProfileArn}` route bind it
    /// as the URI label).
    pub profiles: HashMap<String, ProfileRecord>,
    /// `CreateProfile`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_profile_tokens: TokenLedger<CreateProfileToken>,
    /// `UpgradeProfileVersion`'s `ClientRequestToken` -> the upgrade the original request
    /// performed. Caller input keys it, so it is a BOUNDED [`TokenLedger`]: write it
    /// through [`TokenLedger::remember`].
    pub upgrade_profile_version_tokens: TokenLedger<UpgradeProfileVersionToken>,
    /// Every stored custom lens, keyed by its 32-lowercase-hex lens id — the resource
    /// part of its ARN, which is how a `LensAlias` names it.
    pub lenses: HashMap<String, LensRecord>,
    /// `ImportLens`'s `ClientRequestToken` -> what the original request imported.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`]: write it through
    /// [`TokenLedger::remember`], which holds it at
    /// [`MAX_REMEMBERED_REQUEST_TOKENS`].
    pub import_lens_tokens: TokenLedger<ImportLensToken>,
    /// `CreateLensVersion`'s `ClientRequestToken` -> the version the original request
    /// published. Caller input keys it, so it is a BOUNDED [`TokenLedger`] like every
    /// other ledger here — the lens and version quotas
    /// ([`MAX_CUSTOM_LENSES_PER_ACCOUNT_REGION`] times [`MAX_LENS_VERSIONS_PER_LENS`])
    /// bound what a publish can RECORD, not the stream of tokens a caller can spend
    /// against it.
    pub create_lens_version_tokens: TokenLedger<CreateLensVersionToken>,
    /// Every stored agent profile, keyed by its `AgentProfileArn` — the only identifier
    /// the modelled agent-profile operations take for one (`GetAgentProfile`,
    /// `UpdateAgentProfile`, `DeleteAgentProfile` and every `/goals` route bind
    /// `profileArn` as the URI label).
    pub agent_profiles: HashMap<String, AgentProfileRecord>,
    /// `CreateAgentProfile`'s `clientToken` -> what the original request created. Caller
    /// input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_agent_profile_tokens: TokenLedger<CreateAgentProfileToken>,
    /// `UpdateAgentProfile`'s `clientToken` -> the profile and parameters the original
    /// request applied, so a retry carrying it records no second modification. Caller
    /// input keys it, so it is a BOUNDED [`TokenLedger`] like every ledger here.
    pub update_agent_profile_tokens: TokenLedger<UpdateAgentProfileToken>,
    /// Every optimization goal recorded against an agent profile, keyed by that
    /// profile's ARN then by the goal's `UUID`. A `BTreeMap` because goal-id order is the
    /// stable order `ListAgentGoals` pages over.
    pub agent_goals: HashMap<String, BTreeMap<String, AgentGoalRecord>>,
    /// `CreateAgentGoal`'s `clientToken` -> what the original request created. Caller
    /// input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_agent_goal_tokens: TokenLedger<CreateAgentGoalToken>,
    /// `UpdateAgentGoal`'s `clientToken` -> the goal the original request updated. Held
    /// exactly as the create ledger above is, and bounded for the same reason: caller
    /// input keys it, so write it through [`TokenLedger::remember`].
    pub update_agent_goal_tokens: TokenLedger<UpdateAgentGoalToken>,
    /// Every context recorded against an agent profile, keyed by that profile's ARN then
    /// by the context's `UUID`. A `BTreeMap` because context-id order is the stable order
    /// `ListAgentContexts` pages over.
    pub agent_contexts: HashMap<String, BTreeMap<String, AgentContextRecord>>,
    /// `CreateAgentContext`'s `clientToken` -> what the original request created. Caller
    /// input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_agent_context_tokens: TokenLedger<CreateAgentContextToken>,
    /// Every recommendation generation `StartAgentRecommendationGeneration` has accepted,
    /// keyed by the agent profile's ARN then by the generation's `UUID`. A `BTreeMap`
    /// because generation-id order is the stable order a listing pages over, exactly as
    /// the goals and contexts above are held.
    pub agent_generations: HashMap<String, BTreeMap<String, AgentGenerationRecord>>,
    /// Monotonic input to [`Self::next_hex_id`]; never returned to a caller.
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

    /// A fresh review-template ARN: `review-template/<32 lowercase hex>` in `region`, the
    /// form the modelled `TemplateArn` pattern requires, and unused by any stored
    /// template. The ARN is what [`Self::review_templates`] is keyed by, so the collision
    /// check is ONE lookup — and exact, where a suffix match over the stored ARNs could
    /// also collide across Regions.
    pub fn mint_review_template_arn(&mut self, region: &str) -> String {
        loop {
            let id = self.next_hex_id();
            let arn =
                ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("review-template/{id}"));
            if !self.review_templates.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// A fresh `ProfileArn`: `profile/<32 lowercase hex>` in `region`, which satisfies
    /// the modelled `ProfileArn` pattern's `profile/[a-z0-9]+` resource part, and unused
    /// by any stored profile. The ARN is what [`Self::profiles`] is keyed by, so the
    /// collision check is ONE lookup — and exact, where a suffix match over the stored
    /// ARNs could also collide across Regions.
    ///
    /// The resource part is a minted id rather than the profile's name: the name is a
    /// mutable member (`UpdateProfile`), and an ARN identifies the resource for its whole
    /// life.
    pub fn mint_profile_arn(&mut self, region: &str) -> String {
        loop {
            let id = self.next_hex_id();
            let arn = ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("profile/{id}"));
            if !self.profiles.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// A fresh `ShareId`: 32 lowercase hex characters, as `^[0-9a-f]{32}$` requires, and
    /// unused by any share this store holds. A `ShareId` identifies the share on its own
    /// (`DeleteWorkloadShare` resolves one by id), so uniqueness is checked over every
    /// share of every SHARED RESOURCE rather than within one of them — a workload's and a
    /// lens's shares live in one id space.
    pub fn mint_share_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            if !self.share_id_taken(&id) {
                return id;
            }
        }
    }

    /// Whether any stored share already carries `share_id`.
    fn share_id_taken(&self, share_id: &str) -> bool {
        self.workload_shares
            .values()
            .any(|shares| shares.contains_key(share_id))
            || self
                .lens_shares
                .values()
                .any(|shares| shares.contains_key(share_id))
    }

    /// A fresh `ShareInvitationId`: 32 lowercase hex characters, as `^[0-9a-f]{32}$`
    /// requires, and unused by any invitation this store holds. An invitation is named by
    /// this id alone (`UpdateShareInvitation` resolves one by it), so uniqueness is
    /// checked over every share of every shared resource — and against the `ShareId`s too,
    /// so the two id spaces of one share can never hand out the same value and be mistaken
    /// for each other.
    pub fn mint_share_invitation_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            let taken = self.share_id_taken(&id)
                || self
                    .workload_shares
                    .values()
                    .flat_map(BTreeMap::values)
                    .any(|share| share.share_invitation_id == id)
                || self
                    .lens_shares
                    .values()
                    .flat_map(BTreeMap::values)
                    .any(|share| share.share_invitation_id == id);
            if !taken {
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

    /// A fresh `AgentProfileArn`: `agent-profile/<32 lowercase hex>` in `region`, which
    /// satisfies the modelled pattern's `agent-profile/([a-zA-Z0-9_-]+)` resource part,
    /// and unused by any stored agent profile.
    ///
    /// The resource part is a minted id rather than the profile's `name`: the name is a
    /// mutable member (`UpdateAgentProfile`), and an ARN identifies the resource for its
    /// whole life.
    pub fn mint_agent_profile_arn(&mut self, region: &str) -> String {
        loop {
            let id = self.next_hex_id();
            let arn = ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("agent-profile/{id}"));
            if !self.agent_profiles.contains_key(&arn) {
                return arn;
            }
        }
    }

    /// A fresh goal id: 32 lowercase hex characters in the five dashed groups the
    /// modelled `UUID` pattern requires
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), and unused by
    /// any goal this store holds.
    ///
    /// Uniqueness is checked across EVERY profile's goals, not only one profile's: a goal
    /// id identifies a goal, so two profiles handing out the same one would make the id
    /// ambiguous in anything that reports goals side by side.
    pub fn mint_agent_goal_id(&mut self) -> String {
        loop {
            let id = self.next_uuid();
            if !self
                .agent_goals
                .values()
                .any(|goals| goals.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// The next candidate id in the dashed form the modelled `UUID` pattern requires,
    /// from [`Self::next_hex_id`]'s 32 hex characters. The callers above are what check a
    /// candidate against the resources they mint for.
    fn next_uuid(&mut self) -> String {
        let hex = self.next_hex_id();
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// The goal `id` of the agent profile `profile_arn` names, if that profile holds one.
    pub fn agent_goal(&self, profile_arn: &str, id: &str) -> Option<&AgentGoalRecord> {
        self.agent_goals.get(profile_arn)?.get(id)
    }

    /// Drop one goal of an agent profile and everything keyed to it: any create or update
    /// token that resolved to it, and the profile's goal map once its last goal is gone,
    /// so an emptied profile leaves nothing behind.
    pub fn remove_agent_goal(&mut self, profile_arn: &str, id: &str) -> Option<AgentGoalRecord> {
        let goals = self.agent_goals.get_mut(profile_arn)?;
        let record = goals.remove(id)?;
        if goals.is_empty() {
            self.agent_goals.remove(profile_arn);
        }
        self.create_agent_goal_tokens
            .retain(|token| !(token.profile_arn == profile_arn && token.goal_id == id));
        self.update_agent_goal_tokens
            .retain(|token| !(token.profile_arn == profile_arn && token.goal_id == id));
        Some(record)
    }

    /// [`Self::agent_goal`] for a caller that is about to write to the goal.
    pub fn agent_goal_mut(&mut self, profile_arn: &str, id: &str) -> Option<&mut AgentGoalRecord> {
        self.agent_goals.get_mut(profile_arn)?.get_mut(id)
    }

    /// How many goals the agent profile `profile_arn` names currently holds — what
    /// [`MAX_GOALS_PER_AGENT_PROFILE`] bounds.
    pub fn agent_goal_count(&self, profile_arn: &str) -> usize {
        self.agent_goals
            .get(profile_arn)
            .map_or(0, BTreeMap::len)
    }

    /// A fresh context id, in the five dashed groups the modelled `UUID` pattern requires
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), and unused by
    /// any context this store holds.
    ///
    /// Uniqueness is checked across EVERY profile's contexts, for the reason
    /// [`Self::mint_agent_goal_id`] gives: a context id identifies a context, so two
    /// profiles handing out the same one would make the id ambiguous.
    pub fn mint_agent_context_id(&mut self) -> String {
        loop {
            let id = self.next_uuid();
            if !self
                .agent_contexts
                .values()
                .any(|contexts| contexts.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// The context `id` of the agent profile `profile_arn` names, if that profile holds
    /// one.
    pub fn agent_context(&self, profile_arn: &str, id: &str) -> Option<&AgentContextRecord> {
        self.agent_contexts.get(profile_arn)?.get(id)
    }

    /// How many contexts the agent profile `profile_arn` names currently holds — what
    /// [`MAX_CONTEXTS_PER_AGENT_PROFILE`] bounds.
    pub fn agent_context_count(&self, profile_arn: &str) -> usize {
        self.agent_contexts
            .get(profile_arn)
            .map_or(0, BTreeMap::len)
    }

    /// A fresh recommendation generation id, in the five dashed groups the modelled
    /// `UUID` pattern requires
    /// (`^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`), and unused by
    /// any generation this store holds.
    ///
    /// Uniqueness is checked across EVERY profile's generations, for the reason
    /// [`Self::mint_agent_goal_id`] gives: the id identifies the generation, so two
    /// profiles handing out the same one would make it ambiguous.
    pub fn mint_agent_generation_id(&mut self) -> String {
        loop {
            let id = self.next_uuid();
            if !self
                .agent_generations
                .values()
                .any(|generations| generations.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// The recommendation generation `id` of the agent profile `profile_arn` names, if
    /// that profile holds one. A generation is reached only THROUGH its profile, so the
    /// pair resolves in one lookup.
    pub fn agent_generation(&self, profile_arn: &str, id: &str) -> Option<&AgentGenerationRecord> {
        self.agent_generations.get(profile_arn)?.get(id)
    }

    /// How many recommendation generations the agent profile `profile_arn` names holds —
    /// what [`MAX_GENERATIONS_PER_AGENT_PROFILE`] bounds.
    pub fn agent_generation_count(&self, profile_arn: &str) -> usize {
        self.agent_generations
            .get(profile_arn)
            .map_or(0, BTreeMap::len)
    }

    /// The generation of `profile_arn` that the request fingerprinted `fingerprint`
    /// started and that has not reached a terminal status as of `now`, if there is one.
    ///
    /// `StartAgentRecommendationGeneration` is modelled `@idempotent`
    /// (`smithy_query(query="operation",
    /// operation="StartAgentRecommendationGeneration")`), so repeating a start request
    /// must not leave a second job running: the repeat resolves to the job already in
    /// flight. Once that job is terminal the same request starts a fresh analysis, which
    /// is what asking again then means.
    pub fn in_flight_agent_generation(
        &self,
        profile_arn: &str,
        fingerprint: &str,
        now: SystemTime,
    ) -> Option<&AgentGenerationRecord> {
        self.agent_generations.get(profile_arn)?.values().find(|record| {
            record.fingerprint == fingerprint && !record.is_terminal(now)
        })
    }

    /// The tags recorded on the resource an ARN names, or `None` when it names no
    /// resource this store holds.
    ///
    /// A tag operation names its resource by ARN ALONE, across every taggable resource
    /// type of the service ([`TAGGABLE_RESOURCE_KINDS`]), so ONE lookup spans them all:
    /// the workloads, the custom lenses, the Well-Architected Tool profiles, the review
    /// templates and the agent profiles. Resolved in one place so every operation that
    /// reads or writes a resource's tags agrees about what an ARN names.
    ///
    /// A workload is keyed in this store by its `WorkloadId` rather than by its ARN, so
    /// its ARN is matched over the stored workloads — a scan bounded by
    /// [`MAX_WORKLOADS_PER_ACCOUNT_REGION`]; every other type is keyed by the ARN itself,
    /// so it costs ONE lookup. A lens is resolved through
    /// [`Self::lens_by_alias`], which requires the ARN to agree with the lens's own
    /// (account and Region included) and accepts a version-pinned ARN as naming the same
    /// lens, and it is consulted only for a string that IS an ARN: a bare lens id is not
    /// an ARN, so it names no resource here.
    pub fn resource_tags(&self, resource_arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(record) = self
            .workloads
            .values()
            .find(|record| record.workload_arn == resource_arn)
        {
            return Some(&record.tags);
        }
        if let Some(record) = self.review_templates.get(resource_arn) {
            return Some(&record.tags);
        }
        if let Some(record) = self.profiles.get(resource_arn) {
            return Some(&record.tags);
        }
        if let Some(record) = self.agent_profiles.get(resource_arn) {
            return Some(&record.tags);
        }
        if !resource_arn.starts_with("arn:") {
            return None;
        }
        self.lens_by_alias(resource_arn)
            .map(|record| &record.tags)
    }

    /// The next 32-lowercase-hex candidate id: a per-store counter and the wall clock,
    /// hashed, so it is never derived from the request. The callers above are what check
    /// a candidate against the resources they mint for.
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

    /// The questions the lens a request named asks, in the lens's own order — the ONE
    /// question set every read of a lens review's answers reports and pages over.
    ///
    /// A lens review asks what the LENS's content defines, and this store holds that
    /// content in two halves that never overlap: an AWS-official lens (named by its alias
    /// or by its account-less ARN, [`is_aws_official_lens`]) asks its catalog entry's
    /// questions ([`lens_questions`]), and a custom lens asks the pillars and questions
    /// its imported document defines, as published ([`LensRecord::reviewed_content`] →
    /// [`LensDefinition::review_questions`]).
    ///
    /// Empty only for a lens this emulator holds no content for at all: an official alias
    /// the catalog carries no questions for, a custom lens this store does not hold, or
    /// one no version has been published from — and an unpublished custom lens cannot be
    /// applied to a workload or a review template in the first place
    /// ([`Self::unapplicable_lenses_refusal`]).
    pub fn lens_review_questions(&self, lens: &str) -> Vec<ReviewQuestion> {
        if is_aws_official_lens(lens) {
            return lens_questions(lens)
                .iter()
                .map(FrameworkQuestion::to_review_question)
                .collect();
        }
        self.lens_by_alias(lens)
            .and_then(LensRecord::reviewed_content)
            .map(|content| content.definition.review_questions())
            .unwrap_or_default()
    }

    /// The ONE question the lens a request named asks under `question_id`, when it asks
    /// it: [`Self::lens_review_questions`] resolved by id, which is how every operation
    /// that reads a single answer names the question.
    pub fn lens_review_question(&self, lens: &str, question_id: &str) -> Option<ReviewQuestion> {
        self.lens_review_questions(lens)
            .into_iter()
            .find(|question| question.question_id == question_id)
    }

    /// The `LensArn` the lens a request named is reported by, in the Region the request
    /// was signed for.
    ///
    /// An AWS-official lens's ARN carries no owner account and is built from the alias
    /// ([`official_lens_arn`]), while a custom lens's is the one its import minted and
    /// this store holds. A custom lens this store does not hold is reported by the ARN
    /// the caller named it with: a custom lens cannot be reviewed until
    /// `CreateLensVersion` publishes it — its own stub — so this emulator has no other
    /// ARN for it.
    pub fn lens_arn_of(&self, lens: &str, region: &str) -> String {
        if is_aws_official_lens(lens) {
            return official_lens_arn(region, lens);
        }
        self.lens_by_alias(lens)
            .map_or_else(|| lens.to_string(), |record| record.lens_arn.clone())
    }

    /// The `LensVersion` of the LATEST version of the lens a `LensAlias` names — the
    /// version a review of that lens is CURRENT against, and the one an upgrade of such a
    /// review moves it to.
    ///
    /// An AWS-official lens is published by AWS, so the version of it this emulator ships
    /// content for is its latest ([`lens_metadata`]); a custom lens's latest version is
    /// the one `CreateLensVersion` published most recently
    /// ([`LensRecord::latest_version`]), whatever its draft now holds. `None` for a lens
    /// this emulator holds no content for and for a custom lens no version has been
    /// published from.
    pub fn latest_lens_version(&self, lens: &str) -> Option<&str> {
        if is_aws_official_lens(lens) {
            return lens_metadata(lens).map(|metadata| metadata.version);
        }
        self.lens_by_alias(lens)?
            .latest_version()
            .map(|(version, _)| version)
    }

    /// The `LensName` the lens a `LensAlias` names carries as of `version`: the name the
    /// content of that version gives the lens.
    ///
    /// A custom lens is named BY ITS CONTENT ("name" of the lens document), so a version
    /// published from other content can name it differently and the name is read off the
    /// version a review is against; a version this store does not hold falls back to the
    /// lens's latest, which is also what a review pinning no version is against. An
    /// AWS-official lens's name is the catalog's, which its one shipped version is the
    /// content of.
    pub fn lens_name_at(&self, lens: &str, version: Option<&str>) -> Option<&str> {
        if is_aws_official_lens(lens) {
            return lens_metadata(lens).map(|metadata| metadata.name);
        }
        let record = self.lens_by_alias(lens)?;
        let published = version
            .and_then(|version| record.versions.get(version))
            .or_else(|| record.latest_version().map(|(_, published)| published))?;
        Some(published.content.definition.name.as_str())
    }

    /// The `LensVersion` a review of `lens` created at `now` is against: the version the
    /// lens's content is published AS at that instant.
    ///
    /// A lens review is a review against ONE version of the lens, so applying a lens pins
    /// the version its content is published as right then
    /// ([`LensRecord::published_version`]) — the same way associating a profile with a
    /// workload pins the profile version it is reviewed against
    /// ([`WorkloadProfileRecord::profile_version`]). `None` for a lens this emulator holds
    /// no content for, and for a custom lens whose content is a DRAFT — which no operation
    /// applies, since a custom lens "cannot be applied to workloads … until it's published
    /// with CreateLensVersion" (`ImportLens` doc) is what
    /// [`Self::unapplicable_lenses_refusal`] enforces.
    pub fn applied_lens_version(&self, lens: &str, now: SystemTime) -> Option<String> {
        if is_aws_official_lens(lens) {
            return lens_metadata(lens).map(|metadata| metadata.version.to_string());
        }
        self.lens_by_alias(lens)?
            .published_version(now)
            .map(|(version, _)| version.to_string())
    }

    /// The lens reviews a review template applying `lenses` carries the moment it is
    /// created: one per applied lens, keyed by lens ALIAS, each against the version of
    /// that lens's content as of `now` ([`Self::applied_lens_version`]) and with nothing
    /// written to it yet.
    ///
    /// A lens review exists as soon as the lens is applied
    /// ([`ReviewTemplateRecord::applies_lens`]), which is why the records are created with
    /// the template rather than on the first write to one.
    pub fn applied_lens_reviews(
        &self,
        lenses: &[String],
        now: SystemTime,
    ) -> BTreeMap<String, ReviewTemplateLensReviewRecord> {
        lenses
            .iter()
            .map(|lens| {
                (
                    lens_alias(lens).to_string(),
                    ReviewTemplateLensReviewRecord {
                        notes: None,
                        pillar_notes: BTreeMap::new(),
                        updated_at: now,
                        lens_version: self.applied_lens_version(lens, now),
                    },
                )
            })
            .collect()
    }

    /// What the lens catalog says about the lens ONE lens review of the stored review
    /// template `template_arn` is a review of, for the version that review is against
    /// ([`ReviewTemplateRecord::lens_review_version`]).
    ///
    /// `lens` is the lens as the request named it, which is the spelling its `LensArn` is
    /// resolved from. A `template_arn` this store holds no template under has no review to
    /// report — the caller answers that lookup miss itself — and what comes back then
    /// describes the lens as a review pinning no version would be against, its latest.
    pub fn reviewed_lens(&self, template_arn: &str, lens: &str, region: &str) -> ReviewedLens {
        let pinned = self
            .review_templates
            .get(template_arn)
            .and_then(|template| template.lens_review_version(lens));
        let latest = self.latest_lens_version(lens);
        // The version the review is AGAINST: the one it pins, or the lens's latest for a
        // review that pins none.
        let version = pinned.or(latest);
        ReviewedLens {
            lens_arn: self.lens_arn_of(lens, region),
            lens_name: self.lens_name_at(lens, version).map(str::to_string),
            lens_status: version.zip(latest).map(|(version, latest)| {
                if version == latest {
                    LENS_STATUS_CURRENT.to_string()
                } else {
                    LENS_STATUS_NOT_CURRENT.to_string()
                }
            }),
            lens_version: version.map(str::to_string),
        }
    }

    /// `ReviewTemplate$UpdateStatus` / `ReviewTemplateSummary$UpdateStatus` of one stored
    /// template: [`REVIEW_TEMPLATE_UPDATE_STATUS_LENS_NOT_CURRENT`] while ANY lens review
    /// of it is against a version its lens has superseded, else
    /// [`REVIEW_TEMPLATE_UPDATE_STATUS_CURRENT`].
    ///
    /// Derived on every read from the template's pinned versions and the lens catalog, so
    /// it moves when either does — the template's own view of the `LensStatus` each of its
    /// lens reviews reports.
    pub fn review_template_update_status(&self, template: &ReviewTemplateRecord) -> &'static str {
        let superseded = template.lenses.iter().any(|lens| {
            let pinned = template.lens_review_version(lens);
            matches!(
                (pinned, self.latest_lens_version(lens)),
                (Some(pinned), Some(latest)) if pinned != latest
            )
        });
        if superseded {
            REVIEW_TEMPLATE_UPDATE_STATUS_LENS_NOT_CURRENT
        } else {
            REVIEW_TEMPLATE_UPDATE_STATUS_CURRENT
        }
    }

    /// [`Self::lens_by_alias`] for a caller that is about to write to the lens.
    pub fn lens_by_alias_mut(&mut self, alias: &str) -> Option<&mut LensRecord> {
        let id = lens_alias(alias).to_string();
        let record = self.lenses.get_mut(&id)?;
        alias_names_lens(alias, record).then_some(record)
    }

    /// Drop an agent profile and everything keyed to it: every create or update token
    /// that resolved to it, the goals and contexts recorded against it with the create
    /// tokens that resolved to those, and the recommendation generations started against
    /// it, so nothing outlives the profile it belongs to and the next request carrying
    /// one of those tokens acts afresh.
    ///
    /// `DeleteAgentProfile` "deletes an optimization profile and its associated
    /// configuration": the profile's own configuration — its pillars, aggregation
    /// configuration and tags — lives on the record, and a goal, a context or a
    /// generation is reached only through the profile it belongs to, so none of them can
    /// be left behind. Dropping the generations is also what keeps
    /// [`MAX_GENERATIONS_PER_AGENT_PROFILE`] a real bound on
    /// [`Self::agent_generations`]: that ceiling counts the generations of a LIVE
    /// profile, and an ARN is never minted twice, so a generation left under a deleted
    /// profile's ARN could never be reclaimed.
    pub fn remove_agent_profile(&mut self, profile_arn: &str) -> Option<AgentProfileRecord> {
        let record = self.agent_profiles.remove(profile_arn)?;
        self.create_agent_profile_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.update_agent_profile_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.agent_goals.remove(profile_arn);
        self.create_agent_goal_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.update_agent_goal_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.agent_contexts.remove(profile_arn);
        self.create_agent_context_tokens
            .retain(|token| token.profile_arn != profile_arn);
        self.agent_generations.remove(profile_arn);
        Some(record)
    }

    /// The lens a `LensAlias` names, as of `now`: the whole catalog a lens name resolves
    /// against — the lenses Amazon Web Services publishes
    /// ([`AWS_OFFICIAL_LENS_ALIASES`]) and the custom lenses this account holds
    /// ([`Self::lens_by_alias`]), the latter applicable only once a version has been
    /// published from their content.
    pub fn resolve_lens(&self, lens: &str, now: SystemTime) -> LensResolution {
        // "For custom lenses, this is the lens ARN" (`LensAlias` doc), so a name that
        // carries no owner account can only be an official lens, and one that carries one
        // can only be a custom lens: the two halves of the catalog never overlap.
        if is_aws_official_lens(lens) {
            return if is_published_official_lens(lens) {
                LensResolution::Official
            } else {
                LensResolution::Unknown
            };
        }
        match self.lens_by_alias(lens) {
            None => LensResolution::Unknown,
            Some(record) if record.published_version(now).is_some() => {
                LensResolution::PublishedCustom
            }
            Some(_) => LensResolution::DraftCustom,
        }
    }

    /// How a list of lenses a request asks to APPLY is refused, or `None` when every entry
    /// names a lens that can be applied as of `now`.
    ///
    /// A lens is applied by name, and the name has to resolve: "Each lens is identified by
    /// its `LensSummary$LensAlias`" (`WorkloadLenses` doc), and a custom lens "cannot be
    /// applied to workloads … until it's published with CreateLensVersion" (`ImportLens`
    /// doc). Every operation that attaches lenses to a resource resolves them through here,
    /// so none of them can record a lens another operation would then report a review of
    /// and find nothing for.
    ///
    /// The walk stops at the FIRST entry it refuses and allocates nothing per entry it
    /// accepts, so the work this costs is one map lookup and one scan of the fixed
    /// [`AWS_OFFICIAL_LENS_ALIASES`] catalog per entry and NOTHING that grows with the
    /// caller's list beyond that — the list length itself is bounded by the calling
    /// operation's own lens quota before it gets here.
    pub fn unapplicable_lenses_refusal(
        &self,
        lenses: &[String],
        member: &str,
        now: SystemTime,
    ) -> Option<LensesRefusal> {
        for lens in lenses {
            match self.resolve_lens(lens, now) {
                // An entry naming NO lens is answered as a lookup miss, and one naming a
                // lens that exists and is merely unpublished as a bad member value: told
                // its lens is a draft, a caller would publish a lens this service never
                // found.
                LensResolution::Unknown => {
                    return Some(LensesRefusal::NotFound(unknown_lens_not_found(lens)));
                }
                LensResolution::DraftCustom => {
                    return Some(LensesRefusal::Unapplicable(member_constraint_exception(
                        member,
                        &format!(
                            "member must name a lens with a published version; lens {lens} \
                             has none until one is published with CreateLensVersion"
                        ),
                    )));
                }
                LensResolution::Official | LensResolution::PublishedCustom => {}
            }
        }
        None
    }

    /// The stable list order `ListReviewTemplates` pages over: `TemplateArn`, which is
    /// also what [`Self::review_templates`] is keyed by.
    ///
    /// The model publishes no order for `ReviewTemplates`, so this is the emulator's own.
    /// What it has to be is TOTAL and stable — the `NextToken` carries the ARN of the
    /// last template a page served, so a key that moved or repeated would serve a
    /// template twice or skip it. The ARN is the only member of a template that can do
    /// neither: it is unique and it is fixed for the template's whole life, where
    /// `TemplateName` is a member `UpdateReviewTemplate` can move.
    pub fn review_template_order(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.review_templates.keys().cloned().collect();
        keys.sort();
        keys
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
    ///
    /// [`Self::delete_workload_tokens`] is NOT among them: a token that names the delete
    /// itself has to survive the resource the delete removed, or the retry it was issued
    /// for could not be replayed.
    pub fn remove_workload(&mut self, workload_id: &str) -> Option<WorkloadRecord> {
        let record = self.workloads.remove(workload_id)?;
        self.workload_name_index
            .remove(&normalize_unique_name(&record.workload_name));
        self.create_workload_tokens
            .retain(|token| token.workload_id != workload_id);
        // A milestone belongs to its workload, so the whole set goes with it — and with
        // it every create token that resolved to one of them.
        self.milestones.remove(workload_id);
        self.create_milestone_tokens
            .retain(|token| token.workload_id != workload_id);
        // A share shares THIS workload, so it cannot outlive it either.
        self.workload_shares.remove(workload_id);
        self.create_workload_share_tokens
            .retain(|token| token.workload_id != workload_id);
        // An upgrade token resolves to the upgrade of an association OF this workload, so
        // it goes with the associations the record above carried.
        self.upgrade_profile_version_tokens
            .retain(|token| token.workload_id != workload_id);
        Some(record)
    }

    /// Drop a review template and everything keyed to it: any create token that resolved
    /// to it, so a retry of the create that minted it is no longer a replay of a template
    /// that is gone.
    ///
    /// Everything else a template owns — its lens reviews' notes and the answers recorded
    /// against them — lives INSIDE [`ReviewTemplateRecord`], so it goes with the record
    /// itself. A workload created from a template copied that template's lenses and notes
    /// at create time (`CreateWorkload`), so it keeps them: the workload is not a child of
    /// the template.
    pub fn remove_review_template(&mut self, template_arn: &str) -> Option<ReviewTemplateRecord> {
        let record = self.review_templates.remove(template_arn)?;
        self.create_review_template_tokens
            .retain(|token| token.template_arn != template_arn);
        // An upgrade token resolves to a lens review OF this template, so it cannot
        // outlive it either: the next request carrying one acts afresh.
        self.upgrade_review_template_lens_review_tokens
            .retain(|token| token.resource_id != template_arn);
        Some(record)
    }

    /// The workload already holding `name` under the service's own loose comparison
    /// ("spaces and capitalization are ignored"): its name AS STORED and its id, in the
    /// order [`workload_name_conflict`] takes them. The stored name is what a collision
    /// reports, rather than the caller's spelling of it.
    pub fn workload_holding_name(&self, name: &str) -> Option<(&str, &str)> {
        let workload_id = self.workload_name_index.get(&normalize_unique_name(name))?;
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

    /// The milestone of `workload_id` already holding `name` under the service's own
    /// loose comparison ([`normalize_unique_name`]), if one holds it: "Milestone names
    /// must be unique within a workload" (`smithy_query(query="shape",
    /// shape="MilestoneName")`), so the scope of the lookup is ONE workload and a
    /// spelling differing only in capitalization or whitespace finds the same milestone.
    ///
    /// The record it answers with is the milestone AS STORED, which is what
    /// [`milestone_name_conflict`] reports rather than the caller's spelling of the name.
    pub fn milestone_holding_name(
        &self,
        workload_id: &str,
        name: &str,
    ) -> Option<&MilestoneRecord> {
        let normalized = normalize_unique_name(name);
        self.milestones.get(workload_id)?.values().find(|record| {
            normalize_unique_name(&record.milestone_name) == normalized
        })
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

    /// Every share recorded against `workload_id`, in `ShareId` order.
    pub fn workload_shares_of(&self, workload_id: &str) -> Vec<&WorkloadShareRecord> {
        self.workload_shares
            .get(workload_id)
            .map(|shares| shares.values().collect())
            .unwrap_or_default()
    }

    /// The share `share_id` of `workload_id`, if the workload holds it.
    pub fn workload_share(
        &self,
        workload_id: &str,
        share_id: &str,
    ) -> Option<&WorkloadShareRecord> {
        self.workload_shares.get(workload_id)?.get(share_id)
    }

    /// The share `share_id` of `workload_id` for modification, if the workload holds it.
    /// Resolved through the same two-level key as [`Self::workload_share`], so a share id
    /// is only ever changed within the workload it belongs to.
    pub fn workload_share_mut(
        &mut self,
        workload_id: &str,
        share_id: &str,
    ) -> Option<&mut WorkloadShareRecord> {
        self.workload_shares.get_mut(workload_id)?.get_mut(share_id)
    }

    /// Every share invitation addressed to `principal` that is still waiting on an
    /// answer, in `ShareInvitationId` order — what `ListShareInvitations` reports.
    ///
    /// An invitation is offered to the principal a resource was shared WITH, so a share
    /// is reported to that principal and not to the one that created it; it is
    /// outstanding while its share is still in the status a new share carries
    /// ([`DEFAULT_SHARE_STATUS`], `PENDING`), and is gone once the recipient has
    /// answered it. `ShareInvitationId` order is the stable order the pages are cut on,
    /// which is why the enumeration sorts here rather than at the caller.
    ///
    /// Workload and LENS invitations can exist in this emulator: a profile or
    /// review-template share has no store yet because `CreateProfileShare` and
    /// `CreateTemplateShare` are still stubs with their own issues.
    pub fn pending_share_invitations_for(&self, principal: &str) -> Vec<ShareInvitationSummary> {
        let mut invitations: Vec<ShareInvitationSummary> = self
            .workload_shares
            .values()
            .flat_map(|shares| shares.values())
            .filter(|share| {
                share.shared_with == principal && share.status == DEFAULT_SHARE_STATUS
            })
            .map(|share| {
                let workload_name = self
                    .workloads
                    .get(&share.workload_id)
                    .map(|workload| workload.workload_name.as_str())
                    .unwrap_or_default();
                share.to_invitation_summary(workload_name)
            })
            .collect();
        // A lens share raises an invitation of its own kind, reported through the same
        // enumeration so one order and one `PENDING` rule cover every shared resource.
        let now = SystemTime::now();
        invitations.extend(
            self.lens_shares
                .values()
                .flat_map(|shares| shares.values())
                .filter(|share| {
                    share.shared_with == principal && share.status == DEFAULT_SHARE_STATUS
                })
                .map(|share| {
                    // The lens's NAME is its content's, which a later import can change,
                    // so it is read here rather than copied onto the share.
                    let lens_name = self
                        .lenses
                        .get(&share.lens_id)
                        .and_then(|lens| lens.summary_content(now))
                        .map(|content| content.definition.name.as_str());
                    share.to_invitation_summary(lens_name)
                }),
        );
        invitations.sort_by(|left, right| {
            left.share_invitation_id
                .cmp(&right.share_invitation_id)
        });
        invitations
    }

    /// The share of `workload_id` the principal `shared_with` already holds, if it holds
    /// one: a principal holds at most one share of one workload, so this is what a second
    /// share request for the same principal collides with.
    pub fn workload_share_with(
        &self,
        workload_id: &str,
        shared_with: &str,
    ) -> Option<&WorkloadShareRecord> {
        self.workload_shares
            .get(workload_id)?
            .values()
            .find(|share| share.shared_with == shared_with)
    }

    /// Every share recorded against the lens `lens_id`, in `ShareId` order — the lens
    /// counterpart of [`Self::workload_shares_of`].
    pub fn lens_shares_of(&self, lens_id: &str) -> Vec<&LensShareRecord> {
        self.lens_shares
            .get(lens_id)
            .map(|shares| shares.values().collect())
            .unwrap_or_default()
    }

    /// The share `share_id` of the lens `lens_id`, if the lens holds it.
    pub fn lens_share(&self, lens_id: &str, share_id: &str) -> Option<&LensShareRecord> {
        self.lens_shares.get(lens_id)?.get(share_id)
    }

    /// The share of the lens `lens_id` the principal `shared_with` already holds, if it
    /// holds one: a principal holds at most one share of one lens, so this is what a
    /// second share request for the same principal collides with.
    pub fn lens_share_with(
        &self,
        lens_id: &str,
        shared_with: &str,
    ) -> Option<&LensShareRecord> {
        self.lens_shares
            .get(lens_id)?
            .values()
            .find(|share| share.shared_with == shared_with)
    }

    /// The share the invitation `share_invitation_id` mediates, if it was offered to
    /// `principal` — what `UpdateShareInvitation` resolves, for writing.
    ///
    /// An invitation is named by its own `ShareInvitationId` alone, with no workload in
    /// the request, so the search spans every workload's shares. It is offered to the
    /// principal the resource was shared WITH, so an invitation addressed to anyone else
    /// is not this principal's to answer and resolves to nothing — the same `None` as an
    /// id no share carries, so neither discloses the other.
    pub fn workload_share_by_invitation_mut(
        &mut self,
        share_invitation_id: &str,
        principal: &str,
    ) -> Option<&mut WorkloadShareRecord> {
        self.workload_shares
            .values_mut()
            .flat_map(|shares| shares.values_mut())
            .find(|share| {
                share.share_invitation_id == share_invitation_id
                    && share.shared_with == principal
            })
    }
}

/// The form a name whose uniqueness this service decides LOOSELY is compared in:
/// capitalization and whitespace are dropped, so two spellings that differ only in
/// either are the same name.
///
/// It is the comparison the service documents for a workload name — "The name must be
/// unique within an account within an Amazon Web Services Region. Spaces and
/// capitalization are ignored when checking for uniqueness"
/// (`smithy_query(query="shape", shape="WorkloadName")`) — and it is how every name
/// this service meters for uniqueness is compared, so the rule cannot differ between
/// the resources that carry it: a workload name (through
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::workload_name_index`])
/// and a milestone name within its workload (through
/// [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::milestone_holding_name`]).
pub fn normalize_unique_name(name: &str) -> String {
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

/// The deepest an untyped `Document` member may nest before this service refuses it.
///
/// A `Document` carries whatever JSON the request sent, so its nesting is CALLER-chosen,
/// and every walk of one (fingerprinting it, projecting it) costs one level of nesting:
/// an unbounded document would cost stack depth a caller picks, and stack exhaustion
/// aborts the process rather than failing the call. The ceiling is this emulator's own —
/// far above any realistic `additionalContext` and far below what the stack allows — and
/// a document past it is refused with a modelled 400 by
/// [`document_depth_exceeded`]'s callers.
pub const MAX_DOCUMENT_DEPTH: usize = 32;

/// Whether `document` nests deeper than [`MAX_DOCUMENT_DEPTH`] — the check an operation
/// that stores or fingerprints a caller-supplied `Document` runs BEFORE walking it.
///
/// Iterative, over an explicit worklist, so the check itself costs heap rather than stack
/// however deep the document is.
pub fn document_depth_exceeded(document: Option<&::aws_smithy_types::Document>) -> bool {
    let Some(document) = document else {
        return false;
    };
    let mut pending: Vec<(&::aws_smithy_types::Document, usize)> = vec![(document, 1)];
    while let Some((node, depth)) = pending.pop() {
        if depth > MAX_DOCUMENT_DEPTH {
            return true;
        }
        match node {
            ::aws_smithy_types::Document::Array(items) => {
                pending.extend(items.iter().map(|item| (item, depth + 1)));
            }
            ::aws_smithy_types::Document::Object(members) => {
                pending.extend(members.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    false
}

/// An untyped `Document` member as a JSON value, for [`parameter_fingerprint`]: `null`
/// for a request that carried none, so two requests fingerprint alike exactly when they
/// carry the same document.
///
/// Object members are emitted in SORTED key order, so a document's fingerprint depends
/// neither on the order the wire carried its keys in nor on a `HashMap`'s iteration order
/// (see [`document_json`]).
///
/// The walk is DEPTH-BOUNDED at [`MAX_DOCUMENT_DEPTH`]: nesting is caller-chosen, so the
/// recursion is capped here as well as refused by the handler ahead of it
/// ([`document_depth_exceeded`]), and a node past the cap contributes `null` instead of
/// another frame.
pub fn document_fingerprint(
    document: Option<&::aws_smithy_types::Document>,
) -> ::serde_json::Value {
    document.map_or(::serde_json::Value::Null, |document| {
        document_json(document, 1)
    })
}

/// [`document_fingerprint`]'s walk, carrying the depth of the node it is at.
fn document_json(document: &::aws_smithy_types::Document, depth: usize) -> ::serde_json::Value {
    if depth > MAX_DOCUMENT_DEPTH {
        // Unreachable for a request that got past `document_depth_exceeded`; the cap is
        // restated here so the recursion is bounded by this function alone.
        return ::serde_json::Value::Null;
    }
    match document {
        ::aws_smithy_types::Document::Null => ::serde_json::Value::Null,
        ::aws_smithy_types::Document::Bool(value) => ::serde_json::Value::Bool(*value),
        ::aws_smithy_types::Document::String(value) => {
            ::serde_json::Value::String(value.clone())
        }
        ::aws_smithy_types::Document::Number(number) => match number {
            ::aws_smithy_types::Number::PosInt(value) => ::serde_json::Value::from(*value),
            ::aws_smithy_types::Number::NegInt(value) => ::serde_json::Value::from(*value),
            ::aws_smithy_types::Number::Float(value) => {
                ::serde_json::Number::from_f64(*value).map_or(::serde_json::Value::Null, ::serde_json::Value::Number)
            }
        },
        ::aws_smithy_types::Document::Array(items) => ::serde_json::Value::Array(
            items
                .iter()
                .map(|item| document_json(item, depth + 1))
                .collect(),
        ),
        // Members are emitted in SORTED key order. `Document::Object` is a `HashMap`,
        // whose iteration order differs between two instances holding the same keys, and
        // `serde_json::Map` preserves INSERTION order whenever anything in the workspace
        // enables `serde_json/preserve_order`. Without the sort two renderings of the
        // same document can differ, which would make [`parameter_fingerprint`]
        // non-deterministic and turn an idempotent repeat into a second generation. The
        // same guard the `bedrockruntime` service's `document_to_json` carries.
        ::aws_smithy_types::Document::Object(members) => {
            let mut pairs: Vec<(&String, &::aws_smithy_types::Document)> =
                members.iter().collect();
            pairs.sort_unstable_by_key(|(name, _)| *name);
            ::serde_json::Value::Object(
                pairs
                    .into_iter()
                    .map(|(name, member)| (name.clone(), document_json(member, depth + 1)))
                    .collect(),
            )
        }
    }
}

/// A context's typed content as a JSON value, for [`parameter_fingerprint`]: every
/// member of `ContextContent` in one fixed order, so two requests fingerprint alike
/// exactly when they carry the same content. Lives here because every operation that
/// takes a `ContextContent` under an `@idempotencyToken` has to fingerprint it the same
/// way.
pub fn context_content_fingerprint(content: &ContextContent) -> ::serde_json::Value {
    ::serde_json::json!({
        "accountIds": &content.account_ids,
        "additionalContext": &content.additional_context,
        "applicationOverview": &content.application_overview,
        "applicationType": &content.application_type,
        "architectureOverview": &content.architecture_overview,
        "awsServices": &content.aws_services,
        "criticality": &content.criticality,
        "industry": &content.industry,
        "regions": &content.regions,
        "resourceTags": content.resource_tags.as_ref().map(|tags| {
            tags.iter()
                .map(|tag| ::serde_json::json!({ "key": &tag.key, "value": &tag.value }))
                .collect::<Vec<::serde_json::Value>>()
        }),
        "resourceTypes": &content.resource_types,
    })
}

/// Encode a paging cursor as an opaque, self-describing `NextToken`.
///
/// Hex of the JSON cursor: it satisfies the modelled `NextToken` pattern
/// `^[A-Za-z0-9+/=_-]+$` and carries the whole cursor, so paging needs no
/// server-side token table — a reporting operation must not write to state.
fn encode_cursor(cursor: &::serde_json::Value) -> String {
    hex::encode(cursor.to_string())
}

/// The cursor a `NextToken` carries, or `None` when the token is not one this service
/// issued.
fn decode_cursor(token: &str) -> Option<::serde_json::Value> {
    let bytes = hex::decode(token).ok()?;
    ::serde_json::from_slice(&bytes).ok()
}

/// Encode a `ListWorkloads` cursor: the last `(workload name, workload id)` served.
pub fn encode_next_token(workload_name: &str, workload_id: &str) -> String {
    encode_cursor(&::serde_json::json!({ "n": workload_name, "i": workload_id }))
}

/// The `(workload name, workload id)` a `NextToken` points just past, or `None` when
/// the token is not one this service issued.
pub fn decode_next_token(token: &str) -> Option<(String, String)> {
    let cursor = decode_cursor(token)?;
    let name = cursor.get("n")?.as_str()?.to_string();
    let id = cursor.get("i")?.as_str()?.to_string();
    Some((name, id))
}

/// Encode a `ListWorkloadShares` cursor: the last `ShareId` served, in the same opaque
/// scheme as [`encode_next_token`]. Shares are enumerated in `ShareId` order, so that id
/// is the whole cursor.
pub fn encode_share_next_token(share_id: &str) -> String {
    encode_cursor(&::serde_json::json!({ "s": share_id }))
}

/// The `ShareId` a `ListWorkloadShares` `NextToken` points just past, or `None` when the
/// token is not one this service issued.
pub fn decode_share_next_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("s")?.as_str()?.to_string())
}

/// Encode a `ListShareInvitations` cursor: the last `ShareInvitationId` served, in the
/// same opaque scheme as [`encode_next_token`]. Invitations are enumerated in
/// `ShareInvitationId` order, so that id is the whole cursor. Its own key, so a cursor
/// issued by another paginated operation is not a cursor here.
pub fn encode_share_invitation_next_token(share_invitation_id: &str) -> String {
    encode_cursor(&::serde_json::json!({ "v": share_invitation_id }))
}

/// The `ShareInvitationId` a `ListShareInvitations` `NextToken` points just past, or
/// `None` when the token is not one this service issued.
pub fn decode_share_invitation_next_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("v")?.as_str()?.to_string())
}

/// Encode a `ListMilestones` cursor: the `MilestoneNumber` of the last milestone the
/// page returned. A milestone keeps its number for life and numbers are never reused,
/// so the number alone resumes the enumeration.
pub fn encode_milestone_next_token(milestone_number: i32) -> String {
    encode_cursor(&::serde_json::json!({ "m": milestone_number }))
}

/// The `MilestoneNumber` a `ListMilestones` `NextToken` points just past, or `None`
/// when the token is not one this service issued.
pub fn decode_milestone_next_token(token: &str) -> Option<i32> {
    let cursor = decode_cursor(token)?;
    i32::try_from(cursor.get("m")?.as_i64()?).ok()
}

/// Encode a `ListReviewTemplates` cursor: the `TemplateArn` last served, in the same
/// opaque scheme as [`encode_next_token`]. Templates are enumerated in `TemplateArn`
/// order ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::review_template_order`]),
/// so that ARN is the whole cursor. Its own key, so a cursor issued by another paginated
/// operation is not a cursor here.
pub fn encode_review_template_next_token(template_arn: &str) -> String {
    encode_cursor(&::serde_json::json!({ "t": template_arn }))
}

/// The `TemplateArn` a `ListReviewTemplates` `NextToken` points just past, or `None` when
/// the token is not one this service issued.
pub fn decode_review_template_next_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("t")?.as_str()?.to_string())
}

/// Encode a `ListAgentContexts` cursor: the context `UUID` last served, in the same
/// opaque scheme as [`encode_next_token`]. Contexts are enumerated in id order, so that
/// id is the whole cursor. Its own key, so a cursor issued by another paginated
/// operation is not a cursor here.
pub fn encode_agent_context_next_token(context_id: &str) -> String {
    encode_cursor(&::serde_json::json!({ "x": context_id }))
}

/// The context `UUID` a `ListAgentContexts` `NextToken` points just past, or `None` when
/// the token is not one this service issued.
pub fn decode_agent_context_next_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("x")?.as_str()?.to_string())
}

/// Encode a `ListAgentGoals` cursor: the goal `UUID` last served, in the same opaque
/// scheme as [`encode_next_token`]. Goals are enumerated in id order, so that id is the
/// whole cursor. Its own key, so a cursor issued by another paginated operation is not a
/// cursor here.
pub fn encode_agent_goal_next_token(goal_id: &str) -> String {
    encode_cursor(&::serde_json::json!({ "g": goal_id }))
}

/// The goal `UUID` a `ListAgentGoals` `NextToken` points just past, or `None` when the
/// token is not one this service issued.
pub fn decode_agent_goal_next_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("g")?.as_str()?.to_string())
}

/// Encode a `ListAgentProfiles` cursor: the `AgentProfileArn` last served, in the same
/// opaque scheme as [`encode_next_token`]. Profiles are enumerated in ARN order and an
/// ARN is never reused ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::mint_agent_profile_arn`]),
/// so that ARN is the whole cursor. Its own key, so a cursor issued by another paginated
/// operation is not a cursor here.
pub fn encode_agent_profile_next_token(profile_arn: &str) -> String {
    encode_cursor(&::serde_json::json!({ "p": profile_arn }))
}

/// The `AgentProfileArn` a `ListAgentProfiles` `NextToken` points just past, or `None`
/// when the token is not one this service issued.
pub fn decode_agent_profile_next_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("p")?.as_str()?.to_string())
}

/// What a `NextToken` this service did not issue is answered with. The token is this
/// service's own opaque cursor, so the only thing a caller can do is start the
/// pagination again — which is what `CANNOT_PARSE` tells it. Shared by every paginated
/// operation, so none of them words that refusal differently.
pub const INVALID_NEXT_TOKEN_MESSAGE: &str = "The next token that was passed in is invalid.";

/// The member an undecodable-token finding names (`<Operation>Input$NextToken`).
pub const NEXT_TOKEN_MEMBER: &str = "NextToken";

/// The member a share refusal about the principal offered the share names
/// (`Create<Resource>ShareInput$SharedWith`).
pub const SHARED_WITH_MEMBER: &str = "SharedWith";

/// The sentence a share offered to the principal that is already doing the sharing is
/// refused with.
///
/// A share GRANTS a principal access it does not have — "The owner of a workload can
/// share it with other Amazon Web Services accounts and users"
/// (`smithy_query(query="operation", operation="CreateWorkloadShare")`) — so the refusal
/// is about the RELATIONSHIP between the two principals of the request, not about the
/// value of either one: it states that relationship and names no account id, so a
/// refusal discloses no identity the caller did not send. Shared by every `Create*Share`
/// operation, so none of them words it differently.
pub const SELF_SHARE_MESSAGE: &str =
    "The shared with principal cannot be the same as the sharer's principal.";

/// The modelled `ValidationException` (HTTP 400) for a share offered to the principal
/// doing the sharing: [`SELF_SHARE_MESSAGE`], reported as a finding on the member that
/// named the principal. Unlike [`member_constraint_exception`] it is NOT worded as a
/// per-field constraint — the rule is the relationship between two principals rather
/// than a bound on one member's value — so the sentence stands on its own.
pub fn self_share_exception() -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        SHARED_WITH_MEMBER.to_string(),
        SELF_SHARE_MESSAGE.to_string(),
    )])
}

/// Encode a `ListLenses` cursor: the [`lens_catalog_key`] of the last lens the page
/// served, in the same opaque scheme as [`encode_next_token`]. Its own key, so a cursor
/// issued by another paginated operation is not a cursor here.
pub fn encode_lens_next_token(key: &str) -> String {
    encode_cursor(&::serde_json::json!({ "c": key }))
}

/// The [`lens_catalog_key`] a `ListLenses` `NextToken` points just past, or `None` when
/// the token is not one this service issued.
pub fn decode_lens_next_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("c")?.as_str()?.to_string())
}

/// Encode a `ListLensReviews` cursor: the [`lens_alias`] of the last lens review the page
/// served, in the same opaque scheme as [`encode_next_token`]. A workload's lens reviews
/// are enumerated in lens-alias order and a lens is attached once however it was named,
/// so that alias is the whole cursor. Its own key, so a cursor issued by another
/// paginated operation is not a cursor here.
pub fn encode_lens_review_next_token(alias: &str) -> String {
    encode_cursor(&::serde_json::json!({ "r": alias }))
}

/// The [`lens_alias`] a `ListLensReviews` `NextToken` points just past, or `None` when the
/// token is not one this service issued.
pub fn decode_lens_review_next_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("r")?.as_str()?.to_string())
}

/// Encode a cursor over a lens's questions: the `QuestionId` last served, in the same
/// opaque scheme as [`encode_next_token`].
pub fn encode_question_token(question_id: &str) -> String {
    encode_cursor(&::serde_json::json!({ "q": question_id }))
}

/// The `QuestionId` a questions `NextToken` points just past, or `None` when the token
/// is not one this service issued.
pub fn decode_question_token(token: &str) -> Option<String> {
    Some(decode_cursor(token)?.get("q")?.as_str()?.to_string())
}

/// One page of a lens's questions: the questions to report, and the `NextToken` the next
/// page resumes from when the selection does not fit in this one.
#[derive(Debug)]
pub struct QuestionPage<'a> {
    /// The questions this page reports, in the LENS's own order.
    pub questions: Vec<&'a ReviewQuestion>,
    /// The cursor a caller follows for the rest of the selection, `None` on the last
    /// page.
    pub next_token: Option<String>,
}

/// Page over a lens's question set: the questions `select` keeps, resumed just past the
/// question `resume_after` names and cut to `page_size`.
///
/// Shared by every operation that pages over the answers of a lens review (a workload's
/// or a review template's), so they cannot drift about the order, the cursor or where a
/// page ends. The cursor is start-EXCLUSIVE and names a `QuestionId` in the lens's own
/// order, so `select` can differ between pages without a question being served twice or
/// skipped.
///
/// `None` when `resume_after` names a question the lens does not ask: that is not a
/// cursor this review issued, so it is as undecodable as a malformed token and the
/// caller answers it with its own modelled refusal.
pub fn question_page<'a, Select: Fn(&ReviewQuestion) -> bool>(
    catalog: &'a [ReviewQuestion],
    select: Select,
    resume_after: Option<&str>,
    page_size: usize,
) -> Option<QuestionPage<'a>> {
    let selected: Vec<(usize, &ReviewQuestion)> = catalog
        .iter()
        .enumerate()
        .filter(|(_, question)| select(question))
        .collect();
    let start = match resume_after {
        None => 0,
        Some(question_id) => {
            let position = catalog
                .iter()
                .position(|question| question.question_id == question_id)?;
            selected.partition_point(|(index, _)| *index <= position)
        }
    };
    let remaining = &selected[start.min(selected.len())..];
    let page = &remaining[..remaining.len().min(page_size)];
    let next_token = match page.last() {
        Some((_, question)) if remaining.len() > page.len() => {
            Some(encode_question_token(&question.question_id))
        }
        _ => None,
    };
    Some(QuestionPage {
        questions: page.iter().map(|(_, question)| *question).collect(),
        next_token,
    })
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
    // The distinct messages in the order they first appear, with membership tested against
    // a set rather than by scanning the messages already kept: `fields` carries one entry
    // per offending value of a request list, so a scan per entry would make building the
    // refusal cost work quadratic in a length the caller chose.
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut distinct: Vec<&str> = Vec::new();
    for (_, message) in fields {
        if seen.insert(message.as_str()) {
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

/// The member a refused `SelectedChoices` value is reported under, in either operation
/// that records an answer (`UpdateAnswerInput$SelectedChoices`,
/// `UpdateReviewTemplateAnswerInput$SelectedChoices`).
pub const SELECTED_CHOICES_MEMBER: &str = "SelectedChoices";

/// The member a refused `ChoiceUpdates` key is reported under, in either operation that
/// records an answer. A finding about ONE entry names the path that reaches it
/// (`ChoiceUpdates.<ChoiceId>.<Member>`), so a caller is told WHICH choice's update to fix.
pub const CHOICE_UPDATES_MEMBER: &str = "ChoiceUpdates";

/// The member a refused answer `Reason` is reported under, in either operation that
/// records an answer.
pub const ANSWER_REASON_MEMBER: &str = "Reason";

/// The refusal for the first enum member of an answer-recording request carrying a value
/// outside its modelled value set, if any does: the answer's own `Reason` (an
/// `AnswerReason`) and each `ChoiceUpdate`'s `Status` (a `ChoiceStatus`) and `Reason` (a
/// `ChoiceReason`).
///
/// The boundary's constraint walk does not reach inside a map's values, so these are
/// enforced by the handler — and both operations that record an answer (a workload's and
/// a review template's) take the same two members, so the rule and its wording live here
/// once rather than in each of them.
///
/// The `ChoiceUpdates` entries are judged in `ChoiceId` order, so the member a refusal
/// names does not depend on the request map's iteration order.
pub fn answer_enum_refusal(
    reason: Option<&str>,
    choice_updates: Option<&HashMap<String, crate::ops::types::ChoiceUpdate>>,
) -> Option<crate::ops::types::ValidationException> {
    let mut members: Vec<(String, &str, &[&str])> = Vec::new();
    if let Some(reason) = reason {
        members.push((
            ANSWER_REASON_MEMBER.to_string(),
            reason,
            crate::ops::types::ANSWER_REASON_VALUES,
        ));
    }
    let mut updates: Vec<(&String, &crate::ops::types::ChoiceUpdate)> =
        choice_updates.into_iter().flatten().collect();
    updates.sort_by_key(|(choice_id, _)| *choice_id);
    for (choice_id, update) in updates {
        members.push((
            format!("{CHOICE_UPDATES_MEMBER}.{choice_id}.Status"),
            update.status.as_str(),
            crate::ops::types::CHOICE_STATUS_VALUES,
        ));
        if let Some(reason) = update.reason.as_deref() {
            members.push((
                format!("{CHOICE_UPDATES_MEMBER}.{choice_id}.Reason"),
                reason,
                crate::ops::types::CHOICE_REASON_VALUES,
            ));
        }
    }
    members
        .into_iter()
        .find(|(_, value, allowed)| !allowed.contains(value))
        .map(|(member, _, allowed)| {
            member_constraint_exception(
                &member,
                &format!(
                    "Member must satisfy enum value set: [{}]",
                    allowed.join(", ")
                ),
            )
        })
}

/// The refusal for a `ChoiceId` an answer-recording request names that is not a choice of
/// the question being answered, if either member that carries one does.
///
/// A choice is answered BY ID and the ids are the question's own catalog (`Answer$Choices`
/// / `ReviewTemplateAnswer$Choices`, the only place a client discovers them), so an id
/// naming a choice of another question — or of none — belongs to no answer of this
/// question. Shared by both operations that record an answer, so they cannot word it
/// differently; the offending ids are sorted and deduplicated, so the refusal does not
/// depend on the order the request listed them in.
pub fn undefined_choice_refusal(
    question: &ReviewQuestion,
    selected_choices: Option<&[String]>,
    choice_updates: Option<&HashMap<String, crate::ops::types::ChoiceUpdate>>,
) -> Option<crate::ops::types::ValidationException> {
    // Each member's ids are BORROWED from the request: only the ones the question does
    // not define are collected, so a request whose choices are all valid allocates
    // nothing.
    let named: [(&str, Box<dyn Iterator<Item = &str>>); 2] = [
        (
            SELECTED_CHOICES_MEMBER,
            Box::new(selected_choices.unwrap_or_default().iter().map(String::as_str)),
        ),
        (
            CHOICE_UPDATES_MEMBER,
            Box::new(
                choice_updates
                    .into_iter()
                    .flat_map(|updates| updates.keys().map(String::as_str)),
            ),
        ),
    ];
    for (member, choice_ids) in named {
        let mut undefined: Vec<&str> = choice_ids
            .filter(|choice_id| !question.defines_choice(choice_id))
            .collect();
        if !undefined.is_empty() {
            undefined.sort_unstable();
            undefined.dedup();
            return Some(member_constraint_exception(
                member,
                &format!(
                    "every choice must be a choice of question {}, which {} is not",
                    question.question_id,
                    undefined.join(", ")
                ),
            ));
        }
    }
    None
}

/// The member a refused `PillarNotes` key is reported under, in either operation that
/// records the notes of a lens review (`UpdateLensReviewInput$PillarNotes`,
/// `UpdateReviewTemplateLensReviewInput$PillarNotes`).
pub const PILLAR_NOTES_MEMBER: &str = "PillarNotes";

/// The member a refused Jira question selection is reported under
/// (`UpdateLensReviewInput$JiraConfiguration`). A finding about ONE selected pillar names
/// the path that reaches it (`JiraConfiguration.SelectedPillars.<PillarId>`), so a caller
/// is told WHICH selection to fix.
pub const JIRA_CONFIGURATION_MEMBER: &str = "JiraConfiguration";

/// The refusal for a `PillarId` a request supplies that names no pillar of the lens under
/// review, if any of them does.
///
/// A pillar of a lens is what that lens's content names, so an identifier naming a pillar
/// the lens does NOT define belongs to no pillar of that lens review and is refused rather
/// than stored. Only a lens this emulator holds content for has a pillar set to judge an
/// identifier against ([`defined_pillar_ids`]); for a lens it holds none for there is
/// nothing to judge, so the value is taken as given rather than refused for a pillar the
/// real lens may well define.
///
/// `member` is the member the finding names, and the offending ids are sorted and
/// deduplicated, so the refusal does not depend on the order the request listed them in.
/// Shared by every operation that takes a `PillarId` of a lens under review, so they
/// cannot word it differently.
pub fn undefined_pillar_refusal<'id>(
    member: &str,
    lens: &str,
    pillar_ids: impl Iterator<Item = &'id str>,
) -> Option<crate::ops::types::ValidationException> {
    let defined = defined_pillar_ids(lens);
    if defined.is_empty() {
        return None;
    }
    let mut undefined: Vec<&str> = pillar_ids
        .filter(|pillar_id| !defined.contains(pillar_id))
        .collect();
    if undefined.is_empty() {
        return None;
    }
    undefined.sort_unstable();
    undefined.dedup();
    Some(member_constraint_exception(
        member,
        &format!(
            "every pillar must be a pillar of lens {lens}, which {} is not",
            undefined.join(", ")
        ),
    ))
}

/// The refusal for a `QuestionId` a request supplies that names no question ONE pillar of
/// the lens under review asks, if any of them does.
///
/// The questions of a pillar are what that lens's content names, so an identifier naming a
/// question of another pillar — or of none — belongs to no question of that pillar's
/// review. As with [`undefined_pillar_refusal`], a lens this emulator holds no content for
/// asks nothing here ([`lens_questions`]), so there is no question set to judge an
/// identifier against and the value is taken as given.
pub fn undefined_pillar_question_refusal<'id>(
    member: &str,
    lens: &str,
    pillar_id: &str,
    question_ids: impl Iterator<Item = &'id str>,
) -> Option<crate::ops::types::ValidationException> {
    let asked = lens_questions(lens);
    if asked.is_empty() {
        return None;
    }
    let mut undefined: Vec<&str> = question_ids
        .filter(|question_id| {
            !asked
                .iter()
                .any(|question| question.pillar_id == pillar_id && &question.question_id == question_id)
        })
        .collect();
    if undefined.is_empty() {
        return None;
    }
    undefined.sort_unstable();
    undefined.dedup();
    Some(member_constraint_exception(
        member,
        &format!(
            "every question must be a question pillar {pillar_id} of lens {lens} asks, which {} is not",
            undefined.join(", ")
        ),
    ))
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

/// [`resource_not_found`] for a share id that names no share of the workload.
///
/// A `ShareId` identifies a share on its own — the id space is unique across every
/// workload's shares — so the `ResourceId` is that id AS SENT, and the message names the
/// workload the share was looked for in, so a caller can tell a share of another
/// workload from a share that was revoked. Shared by every operation that resolves a
/// workload `ShareId`, so they cannot word it differently.
pub fn workload_share_not_found(workload_id: &str, share_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        WORKLOAD_SHARE_RESOURCE_TYPE,
        share_id.to_string(),
        &format!("share with ID {share_id} of a workload with ID {workload_id}"),
    )
}

/// [`resource_not_found`] for a `ProfileArn` that names no profile associated with the
/// workload — none is, or the one the workload holds is a different profile.
///
/// A `ProfileArn` identifies the profile on its own, so — as for a workload `ShareId` —
/// the `ResourceId` is that ARN AS SENT, and the message names the workload the profile
/// was looked for in, so a caller can tell a profile of another workload from one that
/// was already disassociated. Shared by every operation that resolves a profile OF a
/// workload, so they cannot word it differently.
pub fn workload_profile_not_found(
    workload_id: &str,
    profile_arn: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        PROFILE_RESOURCE_TYPE,
        profile_arn.to_string(),
        &format!("profile with ARN {profile_arn} associated with a workload with ID {workload_id}"),
    )
}

/// [`conflict`] for a workload that is already reviewed against a profile: a workload
/// reports at most [`MAX_PROFILES_PER_WORKLOAD`] of them, so the association it already
/// holds is what a request associating a DIFFERENT profile collides with — the modelled
/// `ConflictException`'s own "the resource has already been processed" case. `existing_arn`
/// is the association as STORED, so the caller is told which profile to disassociate, and
/// `resource_id` names the workload the collision is inside, as the other
/// association failures of a workload report it.
pub fn workload_profile_conflict(workload_id: &str, existing_arn: &str) -> ConflictException {
    conflict(
        WORKLOAD_RESOURCE_TYPE,
        workload_id.to_string(),
        &format!(
            "The workload with ID {workload_id} is already associated with the profile with \
             ARN {existing_arn}."
        ),
    )
}

/// The refusal for a request that would leave one workload reviewed against more than
/// [`MAX_PROFILES_PER_WORKLOAD`] profiles: the list the association is reported by is
/// modelled `@length(max: 1)`, so the request asks for a result the API cannot carry,
/// whatever the store holds. `Fields` names the member that carried the ARNs.
pub fn too_many_profiles_exception() -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        PROFILE_ARNS_MEMBER.to_string(),
        format!(
            "A workload is associated with at most {MAX_PROFILES_PER_WORKLOAD} profile, so a \
             request must not name more distinct profiles than that."
        ),
    )])
}

/// The refusal for a request whose `LensAliases` is longer than
/// [`MAX_LENS_ALIASES_PER_REQUEST`]: the member publishes no modelled maximum, so this is
/// the quota that keeps the per-request work of an association constant. `named` is the
/// length as sent, so the caller is told how far past the quota it is.
pub fn too_many_lens_aliases_exception(named: usize) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        LENS_ALIASES_MEMBER.to_string(),
        format!(
            "{LENS_ALIASES_MEMBER} must name at most {MAX_LENS_ALIASES_PER_REQUEST} lenses; \
             {named} were named."
        ),
    )])
}

/// The refusal for a request that would leave one workload reviewed against more than
/// [`MAX_LENSES_ASSOCIATED_PER_WORKLOAD`] lenses: the service publishes that maximum, so
/// the request asks for a workload the service does not serve. `attached` is what the
/// workload already holds, so the caller is told how much room is left.
pub fn too_many_lenses_associated_exception(
    workload_id: &str,
    attached: usize,
) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        LENS_ALIASES_MEMBER.to_string(),
        format!(
            "A workload is associated with at most {MAX_LENSES_ASSOCIATED_PER_WORKLOAD} lenses, \
             and the workload with ID {workload_id} is already associated with {attached}."
        ),
    )])
}

/// The refusal for a request whose `ProfileArns` is longer than
/// [`MAX_PROFILE_ARNS_PER_REQUEST`]: the member publishes no modelled maximum, so this is
/// the quota that keeps the per-request work of an association constant. `named` is the
/// length as sent, so the caller is told how far past the quota it is.
pub fn too_many_profile_arns_exception(named: usize) -> crate::ops::types::ValidationException {
    field_validation_exception(&[(
        PROFILE_ARNS_MEMBER.to_string(),
        format!(
            "{PROFILE_ARNS_MEMBER} must name at most {MAX_PROFILE_ARNS_PER_REQUEST} profile \
             ARNs; {named} were named."
        ),
    )])
}

/// [`resource_not_found`] for a template ARN that names no stored review template.
/// Shared by every operation that resolves a `TemplateArn`, so they cannot word it
/// differently.
pub fn review_template_not_found(template_arn: &str) -> ResourceNotFoundException {
    resource_not_found(
        REVIEW_TEMPLATE_RESOURCE_TYPE,
        template_arn.to_string(),
        &format!("review template with ARN {template_arn}"),
    )
}

/// [`resource_not_found`] for a `ProfileArn` that names no stored Well-Architected Tool
/// profile. Shared by every operation that resolves a `ProfileArn`, so they cannot word
/// it differently.
pub fn profile_not_found(profile_arn: &str) -> ResourceNotFoundException {
    resource_not_found(
        PROFILE_RESOURCE_TYPE,
        profile_arn.to_string(),
        &format!("profile with ARN {profile_arn}"),
    )
}

/// [`resource_not_found`] for an ARN a TAG operation names that resolves to no resource
/// this store holds ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::resource_tags`]).
///
/// `ResourceType` is read off the ARN's own resource part
/// ([`TAGGABLE_RESOURCE_KINDS`]), so a caller is told which KIND of resource was looked
/// for; an ARN naming no taggable kind at all is reported under the resource type of the
/// member that carried it (`WorkloadArn`). `ResourceId` is the ARN AS SENT. Shared by
/// every operation that resolves a resource by tag ARN, so they cannot word it
/// differently.
pub fn tagged_resource_not_found(resource_arn: &str) -> ResourceNotFoundException {
    // partition : service : region : account : resource
    let resource_part = resource_arn
        .strip_prefix("arn:")
        .and_then(|rest| rest.splitn(5, ':').nth(4))
        .unwrap_or_default();
    let (resource_type, subject) = TAGGABLE_RESOURCE_KINDS
        .iter()
        .find(|(prefix, _, _)| resource_part.starts_with(prefix))
        .map_or(
            (WORKLOAD_RESOURCE_TYPE, "resource"),
            |(_, resource_type, subject)| (*resource_type, *subject),
        );
    resource_not_found(
        resource_type,
        resource_arn.to_string(),
        &format!("{subject} with ARN {resource_arn}"),
    )
}

/// [`resource_not_found`] for a `ProfileVersion` the profile `profile_arn` names does not
/// have.
///
/// A profile is read AT a version (`GetProfileInput$ProfileVersion`), so a version the
/// profile never had names no readable resource; the `ResourceId` is the profile ARN as
/// sent qualified by the version asked for, so a caller can tell which of the two it got
/// wrong.
pub fn profile_version_not_found(
    profile_arn: &str,
    profile_version: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        PROFILE_RESOURCE_TYPE,
        format!("{profile_arn}.{profile_version}"),
        &format!("version {profile_version} of a profile with ARN {profile_arn}"),
    )
}

/// [`resource_not_found`] for an `AgentProfileArn` that names no stored agent profile.
/// Shared by every operation that resolves one, so they cannot word it differently.
pub fn agent_profile_not_found(profile_arn: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_PROFILE_RESOURCE_TYPE,
        profile_arn.to_string(),
        &format!("agent profile with ARN {profile_arn}"),
    )
}

/// [`conflict`] for a delete of an agent profile whose stored `deletionProtection` is
/// enabled.
///
/// Deletion protection is the profile's own recorded answer to "may this resource be
/// deleted" (`AgentProfileSummary$deletionProtection`, "Indicates whether deletion
/// protection is enabled for the profile"), so a delete of a protected profile is
/// refused by the resource's STATE rather than by anything wrong with the request — a
/// 409 `ConflictException`, not a 400. The sentence names the ARN as sent and the member
/// that has to change first, so a caller can clear the protection and retry.
pub fn agent_profile_deletion_protected(profile_arn: &str) -> ConflictException {
    conflict(
        AGENT_PROFILE_RESOURCE_TYPE,
        profile_arn.to_string(),
        &format!(
            "The agent profile with ARN {profile_arn} cannot be deleted while \
             deletionProtection is enabled."
        ),
    )
}

/// [`resource_not_found`] for a goal id that names no goal of the agent profile
/// `profile_arn`.
///
/// A goal is reached through its profile, so — as for a milestone — the `ResourceId` is
/// the composite of the two and the message names both: a goal id that belongs to
/// ANOTHER profile is as absent here as one that belongs to none.
pub fn agent_goal_not_found(profile_arn: &str, id: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_GOAL_RESOURCE_TYPE,
        agent_goal_resource_id(profile_arn, id),
        &format!("goal with ID {id} in an agent profile with ARN {profile_arn}"),
    )
}

/// How a goal of an agent profile is named in an exception's `ResourceId`: the profile
/// it belongs to and its own id, which together identify it.
pub fn agent_goal_resource_id(profile_arn: &str, id: &str) -> String {
    profile_scoped_resource_id(profile_arn, id)
}

/// How a resource that belongs to an agent profile is named in an exception's
/// `ResourceId`: the profile it hangs off and its own id, which together identify it.
/// Goals, contexts and recommendation generations are all reached through their profile,
/// so the composite is spelled ONCE here and the three builders above call it.
fn profile_scoped_resource_id(profile_arn: &str, id: &str) -> String {
    format!("{profile_arn}.{id}")
}

/// [`resource_not_found`] for a context id that names no context of the agent profile
/// `profile_arn`. Worded exactly as [`agent_goal_not_found`], because a context is
/// reached through its profile the same way: a context id that belongs to ANOTHER
/// profile is as absent here as one that belongs to none.
pub fn agent_context_not_found(profile_arn: &str, id: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_CONTEXT_RESOURCE_TYPE,
        agent_context_resource_id(profile_arn, id),
        &format!("context with ID {id} in an agent profile with ARN {profile_arn}"),
    )
}

/// How a context of an agent profile is named in an exception's `ResourceId`: see
/// [`profile_scoped_resource_id`].
pub fn agent_context_resource_id(profile_arn: &str, id: &str) -> String {
    profile_scoped_resource_id(profile_arn, id)
}

/// [`resource_not_found`] for a generation id that names no recommendation generation of
/// the agent profile `profile_arn`. Worded exactly as [`agent_goal_not_found`], because a
/// generation is reached through its profile the same way: an id that belongs to ANOTHER
/// profile is as absent here as one that belongs to none.
pub fn agent_generation_not_found(profile_arn: &str, id: &str) -> ResourceNotFoundException {
    resource_not_found(
        AGENT_GENERATION_RESOURCE_TYPE,
        agent_generation_resource_id(profile_arn, id),
        &format!(
            "recommendation generation with ID {id} in an agent profile with ARN {profile_arn}"
        ),
    )
}

/// How a recommendation generation of an agent profile is named in an exception's
/// `ResourceId`: see [`profile_scoped_resource_id`].
pub fn agent_generation_resource_id(profile_arn: &str, id: &str) -> String {
    profile_scoped_resource_id(profile_arn, id)
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

/// [`resource_not_found`] for a lens that is not associated with the workload, so the
/// workload has no review of it to report.
///
/// A lens review is identified by the `(WorkloadId, LensAlias)` pair, so — as for a
/// milestone — the `ResourceId` is the composite and the message names both. `lens` is
/// the lens AS SENT, which is how the caller named it.
pub fn lens_review_not_found(workload_id: &str, lens: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_REVIEW_RESOURCE_TYPE,
        lens_review_resource_id(workload_id, lens),
        &format!("review of lens {lens} in a workload with ID {workload_id}"),
    )
}

/// How a lens review is named in an exception's `ResourceId`: the resource it belongs
/// to — a workload's id or a review template's ARN — and the lens it reviews.
pub fn lens_review_resource_id(owner: &str, lens: &str) -> String {
    format!("{owner}.{lens}")
}

/// [`resource_not_found`] for a `QuestionId` the lens under review does not ask, so the
/// review has no answer to that question to read or to record.
///
/// A question of a review is identified by the `(WorkloadId, LensAlias, QuestionId)`
/// triple — the lens decides which questions exist, and the workload decides which lens
/// is reviewed — so the `ResourceId` is the composite and the message names all three.
/// `lens` is the lens AS SENT, which is how the caller named it. Worded like
/// [`review_template_answer_not_found`], the same failure about a review template's lens
/// review.
pub fn question_not_found(
    workload_id: &str,
    lens: &str,
    question_id: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        QUESTION_RESOURCE_TYPE,
        format!("{}.{question_id}", lens_review_resource_id(workload_id, lens)),
        &format!(
            "question with ID {question_id} in a review of lens {lens} in a workload with \
             ID {workload_id}"
        ),
    )
}

/// [`resource_not_found`] for a lens that is not applied to the review template, so the
/// template has no review of it to report.
///
/// Worded like [`lens_review_not_found`] — the same resource type and the same composite
/// `ResourceId` — because it is the same resource: a lens review, here of a template
/// rather than of a workload. `lens` is the lens AS SENT, which is how the caller named
/// it.
pub fn review_template_lens_review_not_found(
    template_arn: &str,
    lens: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        LENS_REVIEW_RESOURCE_TYPE,
        lens_review_resource_id(template_arn, lens),
        &format!("review of lens {lens} in a review template with ARN {template_arn}"),
    )
}

/// `ResourceNotFoundException$ResourceType` for a question of a lens review: the answer
/// to one question is reached through its `QuestionId`, so a `QuestionId` the lens under
/// review does not ask names a missing question rather than a missing lens review. The
/// modelled `ResourceType` is a free-form string (`smithy_query(query="shape",
/// shape="ResourceNotFoundException")`), so this is the emulator's own type name for it,
/// worded like the resource types the model does publish.
pub const QUESTION_RESOURCE_TYPE: &str = "QUESTION";

/// [`resource_not_found`] for a `QuestionId` the lens applied to a review template does
/// not ask, so the template's lens review holds no answer to it to read or write.
///
/// An answer is identified by the review template, the lens and the question together, so
/// — as for a lens review — the `ResourceId` is that composite and the message names all
/// three. `lens` and `question_id` are AS SENT, which is how the caller named them. A lens
/// this emulator holds no content for asks nothing, so every `QuestionId` of such a lens
/// resolves here.
pub fn review_template_answer_not_found(
    template_arn: &str,
    lens: &str,
    question_id: &str,
) -> ResourceNotFoundException {
    resource_not_found(
        QUESTION_RESOURCE_TYPE,
        format!("{}.{question_id}", lens_review_resource_id(template_arn, lens)),
        &format!(
            "question with ID {question_id} in the review of lens {lens} \
             in a review template with ARN {template_arn}"
        ),
    )
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

/// [`resource_not_found`] for a `LensAlias` that names no lens at all: neither a lens
/// Amazon Web Services publishes ([`AWS_OFFICIAL_LENS_ALIASES`]) nor a custom lens this
/// account holds. Distinct from [`lens_not_found`], which is about a name that can only be
/// a CUSTOM lens's and resolved to none.
pub fn unknown_lens_not_found(alias: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("lens with alias {alias}"),
    )
}

/// [`resource_not_found`] for a `LensVersion` of a lens that has not been published under
/// that name. A custom lens carries a version only once `CreateLensVersion` publishes it,
/// and an Amazon Web Services official lens carries the versions this emulator ships
/// content for — so the wording speaks about the lens rather than about how it is owned.
pub fn lens_version_not_found(alias: &str, version: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("version {version} of the lens with alias {alias}"),
    )
}

/// [`resource_not_found`] for a lens no version has been published from, which therefore
/// has no version a caller could read or compare: "The lens cannot be applied to
/// workloads or shared with other Amazon Web Services accounts until it's published with
/// CreateLensVersion" (`ImportLens` doc). The `ResourceId` is the alias AS SENT, as the
/// lookup failures above report it.
pub fn lens_without_published_version(alias: &str) -> ResourceNotFoundException {
    resource_not_found(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("a published version of the lens with alias {alias}"),
    )
}

/// [`conflict`] for a `LensVersion` the lens has already published: a published version
/// is immutable, so a second publish under the same name collides with it rather than
/// replacing it — the modelled `ConflictException`'s own "The resource has already been
/// processed" case. The `ResourceId` is the alias AS SENT, as the lookup failures above
/// report it.
pub fn lens_version_conflict(alias: &str, version: &str) -> ConflictException {
    conflict(
        LENS_RESOURCE_TYPE,
        alias.to_string(),
        &format!("Version {version} of the lens with alias {alias} has already been published."),
    )
}

/// A modelled `ConflictException` (HTTP 409) for a request that collides with a
/// resource that already exists.
///
/// The message names the resource that is in the way AS STORED, and `resource_id`
/// identifies it, so a caller can fetch it. It is deliberately the stored value rather
/// than the spelling the request submitted: the two differ wherever the uniqueness rule
/// compares loosely (a workload name and a milestone name both ignore spaces and
/// capitalization — [`normalize_unique_name`]), and what the caller needs named is the
/// resource it collided with.
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
/// (`smithy_query(query="shape", shape="MilestoneName")`). `existing_name` /
/// `existing_number` are the stored name and number of the milestone already holding it.
///
/// The sentence names the WORKLOAD the uniqueness is scoped to — a milestone name is
/// unique within one workload, not across the account, so the refusal has to say which
/// resource it collided inside — and states the comparison the service applies
/// ([`normalize_unique_name`]), so a caller whose spelling differs only in
/// capitalization or whitespace can tell why it was refused.
pub fn milestone_name_conflict(
    existing_name: &str,
    workload_id: &str,
    existing_number: i32,
) -> ConflictException {
    conflict(
        MILESTONE_RESOURCE_TYPE,
        milestone_resource_id(workload_id, existing_number),
        &format!(
            "Milestone with name {existing_name} already exists under workload \
             {workload_id}. Capitalization and whitespace are ignored in milestone names."
        ),
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

/// [`conflict`] for a principal a workload is already shared with: a share is the grant
/// of one permission on one workload to one principal, so the principal already holding
/// one is what a second share request for it collides with. `existing` is the share that
/// is in the way, which `resource_id` names so the caller can update or delete it.
pub fn workload_share_conflict(existing: &WorkloadShareRecord) -> ConflictException {
    conflict(
        WORKLOAD_SHARE_RESOURCE_TYPE,
        existing.share_id.clone(),
        &format!(
            "The workload with ID {} is already shared with {}.",
            existing.workload_id, existing.shared_with
        ),
    )
}

/// [`conflict`] for a principal a LENS is already shared with — the lens counterpart of
/// [`workload_share_conflict`], for the same reason: a share is the grant of access to
/// one lens to one principal, so the principal already holding one is what a second
/// share request for it collides with. `existing` is the share that is in the way, which
/// `resource_id` names so the caller can delete it.
pub fn lens_share_conflict(existing: &LensShareRecord) -> ConflictException {
    conflict(
        LENS_SHARE_RESOURCE_TYPE,
        existing.share_id.clone(),
        &format!(
            "The lens with ARN {} is already shared with {}.",
            existing.lens_arn, existing.shared_with
        ),
    )
}

/// [`resource_not_found`] for a `ShareInvitationId` that names no invitation this
/// principal has been offered — none is recorded under it, or the one that is was
/// offered to another principal.
///
/// The `ResourceId` is the id AS SENT, which is how the request named the invitation.
pub fn share_invitation_not_found(share_invitation_id: &str) -> ResourceNotFoundException {
    resource_not_found(
        SHARE_INVITATION_RESOURCE_TYPE,
        share_invitation_id.to_string(),
        &format!("share invitation with ID {share_invitation_id}"),
    )
}

/// [`conflict`] for an invitation that is no longer waiting on an answer: the share it
/// mediates has left [`DEFAULT_SHARE_STATUS`], so the invitation has already been
/// processed — the modelled `ConflictException`'s own "The resource has already been
/// processed, was deleted" case. `status` is the `ShareStatus` the share stands in, which
/// is what tells the caller whether it (or someone else) already answered it.
pub fn share_invitation_answered_conflict(
    share_invitation_id: &str,
    status: &str,
) -> ConflictException {
    conflict(
        SHARE_INVITATION_RESOURCE_TYPE,
        share_invitation_id.to_string(),
        &format!(
            "The share invitation with ID {share_invitation_id} is no longer waiting on an \
             answer; the share it mediates is {status}."
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
/// workload" (the `CreateWorkload` doc) — an invariant of the resource, so it guards
/// every call that would leave a stored workload naming no Region. The rule spans both
/// members, so `Fields` carries an entry for EACH of them with the one sentence that
/// states it, and `Message` states that sentence once.
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
