// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-wellarchitected-wellarchitectedapiservicelambda service.
//!
//! The workload lifecycle (`CreateWorkload`, `GetWorkload`, `ListWorkloads`,
//! `UpdateWorkload`, `DeleteWorkload`), the milestones recorded against a workload
//! (`CreateMilestone`, `GetMilestone`, `ListMilestones`), the review templates
//! (`CreateReviewTemplate`, `GetReviewTemplate`), the answers of a workload's lens
//! reviews (`ListAnswers`) and the answers of a review template's lens reviews
//! (`ListReviewTemplateAnswers`) are backed entirely by the maps below, so a create is
//! visible to the next request on the same engine. The custom lenses `ImportLens`
//! imports, the versions `CreateLensVersion` publishes from them and the content
//! `ExportLens` reads back are backed the same way, and `ListLenses` pages over those
//! lenses beside the AWS-official lens this emulator ships. The record types and their
//! projections live here rather than in a handler because several operations reach them.
//!
//! The one lens definition this emulator ships itself — the AWS-official Well-Architected
//! Framework lens's question set — lives here too, for the same reason: the risk roll-up
//! of a workload and the answer list of a lens review must agree about which questions
//! a lens asks.
//!
//! The modelled `ResourceNotFoundException` / `ConflictException` constructors live
//! here for the same reason: every operation that resolves an identifier or enforces a
//! uniqueness rule must word the failure identically, so there is ONE place each
//! message is built.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    Choice, ConflictException, LensSummary, Milestone, MilestoneSummary,
    ResourceNotFoundException,
    ReviewTemplate, ShareInvitationSummary, Workload, WorkloadDiscoveryConfig,
    WorkloadJiraConfigurationInput, WorkloadJiraConfigurationOutput, WorkloadShare,
    WorkloadShareSummary, WorkloadSummary, QUESTION_VALUES, RISK_VALUES,
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

/// The `LensSummary$Description` of the AWS-official Well-Architected Framework lens —
/// what the one lens definition this emulator ships ([`FRAMEWORK_LENS_QUESTIONS`]) asks
/// about, within `LensDescription`'s modelled 1..1024 characters
/// (`smithy_query(query="shape", shape="LensSummary")`). Catalog content rather than a
/// stored value, so it lives beside the question set it describes, next to that lens's
/// name and version ([`FRAMEWORK_LENS_NAME`], [`FRAMEWORK_LENS_VERSION`]).
pub const FRAMEWORK_LENS_DESCRIPTION: &str = "The Amazon Web Services Well-Architected \
     Framework provides a consistent approach for customers and partners to evaluate \
     architectures, and implement designs that scale over time.";

/// When [`FRAMEWORK_LENS_VERSION`] was published, in seconds since the Unix epoch:
/// `2023-10-03T00:00:00Z`, the date that version names.
///
/// `LensSummary$CreatedAt`/`UpdatedAt` are "The date and time when the lens was
/// created"/"last updated", and for AWS's own lens that is when the version this
/// emulator ships was published. Fixed, not read from the clock: a lens nobody changed
/// must report the same instants to every caller.
const FRAMEWORK_LENS_PUBLISHED_AT_EPOCH_SECONDS: u64 = 1_696_291_200;

/// [`FRAMEWORK_LENS_PUBLISHED_AT_EPOCH_SECONDS`] as an instant.
pub fn framework_lens_published_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(FRAMEWORK_LENS_PUBLISHED_AT_EPOCH_SECONDS)
}

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

/// The AWS Well-Architected Framework lens's pillars, in the order the framework
/// presents them: the id each is named by on the wire, the name the framework publishes
/// for it, and the number of questions that pillar asks.
///
/// `PillarId` is how a pillar is named on the wire (`smithy_query(query="shape",
/// shape="PillarId")`: "The ID used to identify a pillar, for example, `security`"), and
/// `PillarName` is its display name (`smithy_query(query="shape",
/// shape="PillarReviewSummary")`) — the six pillar names the Well-Architected Framework
/// publishes. The question counts are the framework's published question numbering —
/// operational excellence OPS 1-11, security SEC 1-11, reliability REL 1-13,
/// performance efficiency PERF 1-5, cost optimization COST 1-11 and sustainability
/// SUS 1-6 — 57 questions in all. This table is the pillar ORDER (it is what a workload
/// without priorities is reviewed in, and the order a lens review reports its pillar
/// summaries in); the questions themselves are [`FRAMEWORK_LENS_QUESTIONS`], and a test
/// pins the two against each other so the counts restated here cannot drift from the
/// catalog.
pub const FRAMEWORK_LENS_PILLARS: &[(&str, &str, usize)] = &[
    ("operationalExcellence", "Operational Excellence", 11),
    ("security", "Security", 11),
    ("reliability", "Reliability", 13),
    ("performance", "Performance Efficiency", 5),
    ("costOptimization", "Cost Optimization", 11),
    ("sustainability", "Sustainability", 6),
];

/// The `LensName` of the AWS-official Well-Architected Framework lens: the name AWS
/// publishes for the lens [`FRAMEWORK_LENS_ALIAS`] names, which is the one lens this
/// emulator ships content for.
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
/// `NOT_CURRENT`, `DEPRECATED`, `DELETED` and `UNSHARED`). The framework lens content
/// here has exactly one version ([`FRAMEWORK_LENS_VERSION`]), and the operations that
/// could move a review off it — `CreateLensVersion`, `DeleteLens`, `CreateLensShare` and
/// `UpgradeLensReview` — are still stubs, each with its own issue, so a review of a lens
/// this emulator holds content for is on that lens's current version.
pub const LENS_STATUS_CURRENT: &str = "CURRENT";

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
    /// [`FRAMEWORK_LENS_PILLARS`].
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

    /// This question's CHOICE catalog — `AnswerSummary$Choices` /
    /// `ReviewTemplateAnswerSummary$Choices` / `Answer$Choices`: one `Choice` per best
    /// practice the question asks about, in the lens's order.
    ///
    /// It is the only place a client can discover the `ChoiceId`s that
    /// `UpdateAnswer$SelectedChoices` and `ChoiceUpdates` name, so every projection of a
    /// question reports it from here and they cannot disagree. `HelpfulResource`,
    /// `ImprovementPlan` and `AdditionalResources` are left off: each "only applies to
    /// custom lenses" / "does not apply to Amazon Web Services official lenses"
    /// (`smithy_query(query="shape", shape="Choice")`), and this is an official lens.
    pub fn choices(&self) -> Vec<Choice> {
        self.best_practices
            .iter()
            .enumerate()
            .map(|(index, best_practice)| Choice {
                choice_id: Some(self.choice_id(index)),
                title: Some((*best_practice).to_string()),
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
}

/// The AWS Well-Architected Framework lens's question set: the one lens definition this
/// emulator ships, in pillar order and then in the framework's question order.
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

/// `ShareStatus` a newly created workload share carries.
///
/// A share is mediated by a workload invitation — "Shared access to a workload is not
/// removed until the workload invitation is deleted" (`CreateWorkloadShare` doc) — so a
/// share the sharer has just created is waiting on the principal it was offered to, and
/// `PENDING` is the `ShareStatus` member for exactly that
/// (`smithy_query(query="shape", shape="ShareStatus")`). Nothing the CLOCK does moves it:
/// it leaves `PENDING` only when the recipient answers the invitation
/// (`UpdateShareInvitation`, still a stub with its own issue).
pub const DEFAULT_SHARE_STATUS: &str = "PENDING";

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
/// because a PROFILE associated with the workload prioritises it. No profile can be
/// associated in this emulator yet — `CreateProfile` and `AssociateProfiles` are stubs,
/// each with its own issue, and `CreateWorkload` refuses a `ProfileArn` with a 404 — so
/// no question of any workload is prioritized.
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

/// `ReviewTemplateUpdateStatus$CURRENT`: what a review template reports while every lens
/// applied to it is at its current version. The enum publishes exactly `CURRENT` and
/// `LENS_NOT_CURRENT` (`smithy_query(query="shape",
/// shape="ReviewTemplateUpdateStatus")`), and a template whose lenses have not been
/// superseded is current. A stored template records the lens aliases applied to it and
/// no lens VERSION ([`ReviewTemplateRecord::lenses`]), so there is no pinned version for
/// a later `CreateLensVersion` to supersede: pinning one is `UpdateReviewTemplate` /
/// `UpgradeReviewTemplateLensReview`'s work, each a stub with its own issue.
pub const REVIEW_TEMPLATE_UPDATE_STATUS_CURRENT: &str = "CURRENT";

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

/// The largest page any paginated operation of this service serves, and the page size a
/// request that omits `MaxResults` gets.
///
/// Every paginated operation here targets the one modelled `MaxResults` shape, which is
/// `@range(min: 1, max: 50)` (`smithy_query(query="shape", shape="MaxResults")`), so
/// one ceiling covers them all: a request that omits the member asks for a full page.
pub const MAX_RESULTS_CEILING: i32 = 50;

/// [`MAX_RESULTS_CEILING`] applied to the `MaxResults` a request carried: the modelled
/// range restated in the handler, so the page size is in `1..=MAX_RESULTS_CEILING`
/// whatever reaches it.
pub fn page_size(max_results: Option<i32>) -> usize {
    let clamped = max_results
        .unwrap_or(MAX_RESULTS_CEILING)
        .clamp(1, MAX_RESULTS_CEILING);
    usize::try_from(clamped)
        .expect("MaxResults is clamped into 1..=MAX_RESULTS_CEILING, which always fits in a usize")
}

/// One stored answer to one question of one lens review, as the operations that answer
/// a question record it.
///
/// Only the members a projection reports are held: `AnswerSummary` carries the selected
/// choices, the per-choice answers, whether the question applies and the reason it does
/// not. The question's RISK is not stored — it is derived from these by
/// [`AnswerRecord::risk`] on every read, so it moves when the answer does.
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
}

/// One stored per-choice answer: the choice's status and the reason for it.
#[derive(Debug, Clone)]
pub struct ChoiceAnswerRecord {
    /// A `ChoiceStatus` (`SELECTED` | `UNSELECTED` | `NOT_APPLICABLE`).
    pub status: String,
    /// A `ChoiceReason`, when one was given.
    pub reason: Option<String>,
}

impl AnswerRecord {
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
    /// The answers recorded against this workload's lens reviews: lens ALIAS (as
    /// [`lens_alias`] resolves it, so one entry however the request named the lens),
    /// then `QuestionId`. An answer lives on the workload rather than in a map of its
    /// own because a milestone records the workload BY VALUE: freezing the workload
    /// freezes the answers it had, which is what a milestone read reports.
    pub answers: BTreeMap<String, BTreeMap<String, AnswerRecord>>,
}

impl WorkloadRecord {
    /// The answer this workload records for one question of one lens, if any.
    ///
    /// `lens` is a lens as a request names it — an alias or an ARN — and resolves
    /// through [`lens_alias`], so the same answer is found by either name.
    pub fn answer(&self, lens: &str, question_id: &str) -> Option<&AnswerRecord> {
        self.answers.get(lens_alias(lens))?.get(question_id)
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
        // A lens is attached once however many ways the request named it, so resolve
        // each name to its lens and count that lens's questions once.
        let attached: std::collections::BTreeSet<&str> =
            self.lenses.iter().map(|lens| lens_alias(lens)).collect();
        for lens in attached {
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
            // the share invitation the recipient was offered. A share can be created
            // (`CreateWorkloadShare`) and the invitations it raises are reported
            // (`ListShareInvitations`), but accepting one — `UpdateShareInvitation` — is
            // still a stub with its own issue, so no workload in this store is one the
            // caller reached through an accepted invitation.
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

/// A risk roll-up with one entry for every `Risk` the enum publishes, all zero: what
/// every roll-up here starts from, so a caller that reads a bucket to report review
/// progress always finds it.
fn empty_risk_counts() -> BTreeMap<String, i32> {
    RISK_VALUES
        .iter()
        .map(|risk| ((*risk).to_string(), 0))
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
}

impl ReviewTemplateRecord {
    /// The answer this template records for one question of one lens, if any.
    ///
    /// `lens` is a lens as a request names it — an alias or an ARN — and resolves
    /// through [`lens_alias`], so the same answer is found by either name.
    pub fn answer(&self, lens: &str, question_id: &str) -> Option<&AnswerRecord> {
        self.answers.get(lens_alias(lens))?.get(question_id)
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
        let mut counts: BTreeMap<String, i32> = QUESTION_VALUES
            .iter()
            .map(|bucket| ((*bucket).to_string(), 0))
            .collect();
        let applied: std::collections::BTreeSet<&str> =
            self.lenses.iter().map(|lens| lens_alias(lens)).collect();
        for lens in applied {
            for question in lens_questions(lens) {
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
        }
        counts
    }

    /// The `ReviewTemplate` shape `GetReviewTemplate` returns.
    pub fn to_review_template(&self) -> ReviewTemplate {
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
            update_status: Some(REVIEW_TEMPLATE_UPDATE_STATUS_CURRENT.to_string()),
            // "The ID assigned to the template share invitation": only a template reached
            // through a share invitation carries one, and template shares are still a
            // stub.
            share_invitation_id: None,
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

/// The `LensSummary` of the one AWS-official lens this emulator ships, in the Region the
/// request was signed for.
///
/// The framework lens's content is this emulator's own ([`FRAMEWORK_LENS_QUESTIONS`]), so
/// its summary comes from that catalog rather than from the lens store, which holds only
/// the custom lenses `ImportLens` imported. Its name and version come from
/// [`lens_metadata`], the one projection of the shipped lens's metadata — so this summary
/// cannot name the lens differently from the lens reviews that report the same lens — and
/// only the description and the publication instant are facts the catalog adds here.
///
/// An official lens ARN names no owner account — "the lens ARN, such as
/// `arn:aws:wellarchitected:us-east-1::lens/serverless`" (`LensAlias` doc), the same
/// distinction [`is_aws_official_lens`] reads — so the summary reports no `Owner` either:
/// AWS publishes the lens rather than an account owning it, and `Owner` is modelled an
/// `AwsAccountId` (`^[0-9]{12}$`), which no placeholder would satisfy honestly.
pub fn framework_lens_summary(region: &str) -> LensSummary {
    let published_at = ::aws_smithy_types::DateTime::from(framework_lens_published_at());
    let metadata = lens_metadata(FRAMEWORK_LENS_ALIAS);
    LensSummary {
        lens_arn: Some(official_lens_arn(region, FRAMEWORK_LENS_ALIAS)),
        lens_alias: Some(FRAMEWORK_LENS_ALIAS.to_string()),
        lens_name: metadata.map(|metadata| metadata.name.to_string()),
        lens_type: Some(LENS_TYPE_AWS_OFFICIAL.to_string()),
        description: Some(FRAMEWORK_LENS_DESCRIPTION.to_string()),
        created_at: Some(published_at),
        updated_at: Some(published_at),
        lens_version: metadata.map(|metadata| metadata.version.to_string()),
        owner: None,
        // AWS's own lens is published content: its content IS the version it is published
        // at.
        lens_status: Some(LENS_STATUS_PUBLISHED.to_string()),
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

/// The questions a lens ATTACHED TO A WORKLOAD asks, as this emulator knows it, in the
/// lens's own order.
///
/// Only the AWS-official Well-Architected Framework lens has a definition here
/// ([`FRAMEWORK_LENS_QUESTIONS`]); another lens — an AWS-published specialised lens, or
/// a custom one — carries no question set in this emulator yet, so it asks nothing. A
/// custom lens is not a counter-example even once `CreateLensVersion` has published a
/// version of it: its question set is lens CONTENT, held on its own
/// [`LensRecord`] in the store rather than in this static catalog, and
/// "the lens cannot be applied to workloads … until it's published with
/// CreateLensVersion" (`ImportLens` doc) is enforced by `AssociateLenses`, still a stub
/// with its own issue — so no custom lens in this emulator is attached to a workload for
/// its questions to be reported or counted.
pub fn lens_questions(lens: &str) -> &'static [FrameworkQuestion] {
    if lens_alias(lens) == FRAMEWORK_LENS_ALIAS {
        FRAMEWORK_LENS_QUESTIONS
    } else {
        &[]
    }
}

/// How many questions the named lens asks: the length of its [`lens_questions`], so the
/// roll-up and the answer list cannot disagree about the question set.
pub fn lens_question_count(lens: &str) -> usize {
    lens_questions(lens).len()
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
    FRAMEWORK_LENS_PILLARS
        .iter()
        .map(|(pillar_id, _, _)| (*pillar_id).to_string())
        .collect()
}

/// The pillars the named lens is organized into, in the lens's own order, each with its
/// `PillarName` — the pillar half of [`lens_questions`].
///
/// Only the AWS-official Well-Architected Framework lens has content here
/// ([`FRAMEWORK_LENS_PILLARS`]); any other lens carries no question set in this emulator
/// yet (see [`lens_questions`] for why), so it is organized into no pillars.
pub fn lens_pillars(lens: &str) -> &'static [(&'static str, &'static str, usize)] {
    if lens_alias(lens) == FRAMEWORK_LENS_ALIAS {
        FRAMEWORK_LENS_PILLARS
    } else {
        &[]
    }
}

/// What this emulator knows ABOUT a lens, as a lens review reports it: the members that
/// describe the LENS itself rather than the answers recorded against it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LensMetadata {
    /// `LensReview$LensName`.
    pub name: &'static str,
    /// `LensReview$LensVersion`.
    pub version: &'static str,
    /// `LensReview$LensStatus`.
    pub status: &'static str,
}

/// The named lens's own metadata, when this emulator holds that lens's content.
///
/// `None` for any lens but the AWS-official framework lens: a lens's name, version and
/// status are lens CONTENT, and this emulator ships one lens definition (see
/// [`lens_questions`] for why), so for another lens there is nothing to report and the
/// optional members are omitted rather than guessed at.
pub fn lens_metadata(lens: &str) -> Option<LensMetadata> {
    if lens_alias(lens) == FRAMEWORK_LENS_ALIAS {
        Some(LensMetadata {
            name: FRAMEWORK_LENS_NAME,
            version: FRAMEWORK_LENS_VERSION,
            status: LENS_STATUS_CURRENT,
        })
    } else {
        None
    }
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
}

/// What a `CreateWorkloadShare` idempotency token resolves to: the share the original
/// request created, plus that request's parameter fingerprint.
#[derive(Debug, Clone)]
pub struct CreateWorkloadShareToken {
    pub workload_id: String,
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
    /// Normalized workload name -> `WorkloadId`. Backs the documented "name must be
    /// unique within an account within a Region; spaces and capitalization are
    /// ignored" rule.
    pub workload_name_index: HashMap<String, String>,
    /// `CreateWorkload`'s `ClientRequestToken` -> what the original request created.
    /// Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_workload_tokens: TokenLedger<CreateWorkloadToken>,
    /// Every stored review template, keyed by its `TemplateArn` — the only identifier
    /// the modelled operations take for one (`GetReviewTemplate`,
    /// `UpdateReviewTemplate` and `DeleteReviewTemplate` all bind `TemplateArn` as the
    /// URI label).
    pub review_templates: HashMap<String, ReviewTemplateRecord>,
    /// `CreateReviewTemplate`'s `ClientRequestToken` -> what the original request
    /// created. Caller input keys it, so it is a BOUNDED [`TokenLedger`].
    pub create_review_template_tokens: TokenLedger<CreateReviewTemplateToken>,
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

    /// A fresh `ShareId`: 32 lowercase hex characters, as `^[0-9a-f]{32}$` requires, and
    /// unused by any share this store holds. A `ShareId` identifies the share on its own
    /// (`DeleteWorkloadShare` resolves one by id), so uniqueness is checked over every
    /// workload's shares rather than within one workload.
    pub fn mint_share_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            if !self
                .workload_shares
                .values()
                .any(|shares| shares.contains_key(&id))
            {
                return id;
            }
        }
    }

    /// A fresh `ShareInvitationId`: 32 lowercase hex characters, as `^[0-9a-f]{32}$`
    /// requires, and unused by any invitation this store holds. An invitation is named by
    /// this id alone (`UpdateShareInvitation` resolves one by it), so uniqueness is
    /// checked over every workload's shares — and against the `ShareId`s too, so the two
    /// id spaces of one share can never hand out the same value and be mistaken for each
    /// other.
    pub fn mint_share_invitation_id(&mut self) -> String {
        loop {
            let id = self.next_hex_id();
            if !self.workload_shares.values().any(|shares| {
                shares.contains_key(&id)
                    || shares
                        .values()
                        .any(|share| share.share_invitation_id == id)
            }) {
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

    /// [`Self::lens_by_alias`] for a caller that is about to write to the lens.
    pub fn lens_by_alias_mut(&mut self, alias: &str) -> Option<&mut LensRecord> {
        let id = lens_alias(alias).to_string();
        let record = self.lenses.get_mut(&id)?;
        alias_names_lens(alias, record).then_some(record)
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
    /// Only workload invitations can exist in this emulator so far: a lens, profile or
    /// review-template share has no store because `CreateLensShare`,
    /// `CreateProfileShare` and `CreateTemplateShare` are still stubs with their own
    /// issues.
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
    pub questions: Vec<&'a FrameworkQuestion>,
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
pub fn question_page<'a, Select: Fn(&FrameworkQuestion) -> bool>(
    catalog: &'a [FrameworkQuestion],
    select: Select,
    resume_after: Option<&str>,
    page_size: usize,
) -> Option<QuestionPage<'a>> {
    let selected: Vec<(usize, &FrameworkQuestion)> = catalog
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
            Some(encode_question_token(question.question_id))
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
