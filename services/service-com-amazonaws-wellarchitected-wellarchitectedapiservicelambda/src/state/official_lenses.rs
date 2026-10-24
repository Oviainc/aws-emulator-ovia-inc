// SPDX-License-Identifier: CC0-1.0

//! The catalog of AWS-OFFICIAL lenses this emulator publishes, and the one lookup every
//! operation that reports or counts a lens's content goes through.
//!
//! `LensType$AWS_OFFICIAL` is "a lens Amazon Web Services publishes"
//! (`smithy_query(query="shape", shape="LensType")`), and the service publishes a whole
//! catalog of them rather than one: the Well-Architected Framework lens beside the
//! specialised lenses AWS publishes for a workload type or an industry, each with its own
//! `LensName`, `LensVersion`, description, pillar set and question set — the `LensAlias`
//! doc names `serverless` ("the lens alias, such as `serverless`, or the lens ARN, such
//! as `arn:aws:wellarchitected:us-east-1::lens/serverless`") as one such lens beside the
//! framework's own alias.
//!
//! So a lens's content is looked up BY ALIAS in [`OFFICIAL_LENS_CATALOG`] rather than
//! compared against one alias: `ListLenses` pages over the catalog, a workload's lens
//! review reports the catalog entry's name, version and pillars, the risk roll-up counts
//! that entry's questions and `ListAnswers` lists them. One table, so those five readings
//! of "what does this lens ask?" cannot disagree.
//!
//! ## Where the content comes from
//!
//! The ALIAS and the NAME of each lens are what AWS publishes for it in the
//! Well-Architected lens catalog (<https://docs.aws.amazon.com/wellarchitected/latest/userguide/lenses.html>);
//! the alias is how `LensAlias` names the lens on the wire and the name is what
//! `ListLenses`'s `LensName` filter selects on. Every alias here is one of
//! [`super::AWS_OFFICIAL_LENS_ALIASES`] — the aliases AWS publishes, which is what a lens
//! NAME resolves against when a lens is applied to a workload
//! ([`super::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::resolve_lens`])
//! — and this catalog is the subset of them this emulator carries CONTENT for: a published
//! alias with no entry here names a lens this emulator knows exists and holds no questions
//! for, which is therefore applicable but not listed, because a `LensSummary` is content
//! this emulator would have to invent. A test pins that containment.
//!
//! Everything below the lens — its pillars, its questions and the best practices each
//! question asks about — is lens CONTENT, which neither the Smithy model nor the service
//! docs publish in a machine-readable form, so it is this emulator's own, exactly as the
//! framework lens's `question_id`s already are (see
//! [`super::FRAMEWORK_LENS_QUESTIONS`]): the framework lens carries the framework's
//! published questions and best practices, and each specialised lens carries a question
//! set written from that lens's published focus areas, under this emulator's own stable
//! ids. [`SPECIALISED_LENS_VERSION`] is what every specialised entry reports as its
//! `LensVersion`, and it names THIS catalog's revision of that content — the same reading
//! as [`super::FRAMEWORK_LENS_VERSION`].
//!
//! The catalog is extensible by construction: a lens AWS publishes that is not listed here
//! is simply a lens this emulator holds no content for, and adding it is adding a row.

use std::time::{Duration, SystemTime};

use crate::ops::types::LensSummary;

use super::{
    is_aws_official_lens, lens_alias, official_lens_arn, q, FrameworkQuestion,
    FRAMEWORK_LENS_ALIAS, FRAMEWORK_LENS_DESCRIPTION, FRAMEWORK_LENS_NAME,
    FRAMEWORK_LENS_QUESTIONS, FRAMEWORK_LENS_VERSION, LENS_STATUS_CURRENT,
    LENS_STATUS_PUBLISHED, LENS_TYPE_AWS_OFFICIAL,
};

/// The six pillars of the AWS Well-Architected Framework, in the order the framework
/// presents them: the `PillarId` each is named by on the wire ("The ID used to identify a
/// pillar, for example, `security`" — `smithy_query(query="shape", shape="PillarId")`) and
/// the `PillarName` the framework publishes for it
/// (`smithy_query(query="shape", shape="PillarReviewSummary")`).
///
/// Every lens AWS publishes is organized into these pillars, so this table is shared by
/// the whole catalog: what differs per lens is WHICH questions it asks in each pillar,
/// which is why [`OfficialLens::pillars`] derives a lens's pillar set (and each pillar's
/// question count) from that lens's own question table rather than restating it.
///
/// It is also the pillar ORDER: the order a lens review reports its pillar summaries in,
/// and the priority order a workload that names none is reviewed in
/// ([`super::default_pillar_priorities`]).
pub const WELL_ARCHITECTED_PILLARS: &[(&str, &str)] = &[
    ("operationalExcellence", "Operational Excellence"),
    ("security", "Security"),
    ("reliability", "Reliability"),
    ("performance", "Performance Efficiency"),
    ("costOptimization", "Cost Optimization"),
    ("sustainability", "Sustainability"),
];

/// The `LensVersion` every SPECIALISED AWS-official lens in this catalog reports: the
/// revision of this emulator's content for those lenses.
///
/// It names THIS catalog's content, not a version string AWS publishes (the model
/// publishes none, and the lens content here is this emulator's own) — the same reading as
/// [`super::FRAMEWORK_LENS_VERSION`], which names the revision of the framework lens's
/// question set. A change to a specialised lens's questions changes it. `LensVersion` is
/// `@length(min: 1, max: 32)` (`smithy_query(query="shape", shape="LensReview")`), and a
/// date names a revision inside that.
pub const SPECIALISED_LENS_VERSION: &str = "2025-01-15";

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

/// One AWS-official lens of [`OFFICIAL_LENS_CATALOG`]: the lens AWS publishes, and the
/// content this emulator carries for it.
#[derive(Debug, Clone, Copy)]
pub struct OfficialLens {
    /// The `LensAlias` the lens is named by on the wire, and the last segment of its
    /// account-less lens ARN ([`official_lens_arn`]).
    pub alias: &'static str,
    /// `LensSummary$LensName` / `LensReview$LensName` — the full name AWS publishes for
    /// the lens, which is what `ListLenses$LensName` selects on.
    pub name: &'static str,
    /// `LensSummary$LensVersion` / `LensReview$LensVersion`: the revision of the content
    /// below. A date, so [`Self::published_at`] can report the instant that version names.
    pub version: &'static str,
    /// `LensSummary$Description` — what this lens asks about, within `LensDescription`'s
    /// modelled 1..1024 characters (`smithy_query(query="shape", shape="LensSummary")`).
    pub description: &'static str,
    /// The questions this lens asks, in the lens's own order (pillar by pillar, in
    /// [`WELL_ARCHITECTED_PILLARS`] order): what `ListAnswers` lists and what every risk
    /// and question roll-up of a review of this lens counts over.
    pub questions: &'static [FrameworkQuestion],
}

impl OfficialLens {
    /// The pillars this lens is organized into, in the framework's pillar order, each with
    /// its `PillarName` and the number of questions THIS lens asks in it.
    ///
    /// Derived from [`Self::questions`], so a pillar's count cannot drift from the
    /// questions it holds. A pillar this lens asks nothing in is not a pillar it is
    /// organized into, so it is left out rather than reported with an empty roll-up.
    pub fn pillars(&self) -> Vec<(&'static str, &'static str, usize)> {
        // ONE pass over the questions, accumulating into a counter per pillar of the
        // table, so the cost is the question count rather than the question count per
        // pillar — and the accumulator is sized FROM the table, so a pillar added to it
        // is counted without touching this.
        let mut asked = vec![0usize; WELL_ARCHITECTED_PILLARS.len()];
        for question in self.questions {
            if let Some(slot) = WELL_ARCHITECTED_PILLARS
                .iter()
                .position(|(pillar_id, _)| *pillar_id == question.pillar_id)
                .and_then(|index| asked.get_mut(index))
            {
                *slot = slot.saturating_add(1);
            }
        }
        WELL_ARCHITECTED_PILLARS
            .iter()
            .zip(asked)
            .filter_map(|((pillar_id, pillar_name), count)| {
                (count > 0).then_some((*pillar_id, *pillar_name, count))
            })
            .collect()
    }

    /// This lens's own name, version and status, as a lens review reports them.
    ///
    /// The status is [`LENS_STATUS_CURRENT`]: an AWS-published lens's content IS the
    /// version AWS publishes it at, so a review of the content this catalog carries is a
    /// review of that lens's current version.
    pub fn metadata(&self) -> LensMetadata {
        LensMetadata {
            name: self.name,
            version: self.version,
            status: LENS_STATUS_CURRENT,
        }
    }

    /// When the version this lens reports was published: midnight UTC of the day
    /// [`Self::version`] names.
    ///
    /// `LensSummary$CreatedAt`/`UpdatedAt` are "The date and time when the lens was
    /// created"/"last updated", and for a lens AWS publishes that is when the version this
    /// emulator carries was published. Derived from the version rather than stored beside
    /// it, so the two cannot name different days, and read from the version rather than
    /// from the clock: a lens nobody changed must report the same instants to every caller.
    pub fn published_at(&self) -> SystemTime {
        midnight_utc(self.version)
    }

    /// The `LensSummary` `ListLenses` reports this lens as, in the Region the request was
    /// signed for.
    ///
    /// An official lens ARN names no owner account — "the lens ARN, such as
    /// `arn:aws:wellarchitected:us-east-1::lens/serverless`" (`LensAlias` doc), the same
    /// distinction [`is_aws_official_lens`] reads — so the summary reports no `Owner`
    /// either: AWS publishes the lens rather than an account owning it, and `Owner` is
    /// modelled an `AwsAccountId` (`^[0-9]{12}$`), which no placeholder would satisfy
    /// honestly.
    pub fn summary(&self, region: &str) -> LensSummary {
        let published_at = ::aws_smithy_types::DateTime::from(self.published_at());
        LensSummary {
            lens_arn: Some(official_lens_arn(region, self.alias)),
            lens_alias: Some(self.alias.to_string()),
            lens_name: Some(self.name.to_string()),
            lens_type: Some(LENS_TYPE_AWS_OFFICIAL.to_string()),
            description: Some(self.description.to_string()),
            created_at: Some(published_at),
            updated_at: Some(published_at),
            lens_version: Some(self.version.to_string()),
            owner: None,
            // AWS's own lenses are published content: their content IS the version they
            // are published at.
            lens_status: Some(LENS_STATUS_PUBLISHED.to_string()),
        }
    }
}

/// Midnight UTC of the day `day` names (`YYYY-MM-DD`), as an instant.
///
/// The calendar conversion comes from the maintained library
/// (`aws_smithy_types::DateTime`) rather than from arithmetic written here. TOTAL: text
/// that is not a date answers the epoch rather than panicking, and a test pins every
/// version in the catalog against the day it names so an unparsable one cannot pass
/// unnoticed.
fn midnight_utc(day: &str) -> SystemTime {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{day}T00:00:00Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    );
    debug_assert!(
        parsed.is_ok(),
        "a lens version must name a calendar day: {day:?}"
    );
    parsed
        .ok()
        .and_then(|at| u64::try_from(at.secs()).ok())
        .map_or(SystemTime::UNIX_EPOCH, |secs| {
            SystemTime::UNIX_EPOCH + Duration::from_secs(secs)
        })
}

/// The lens the given `LensAlias` names in this catalog, if this catalog carries it.
///
/// A lens is named either by its alias or by its ARN ([`lens_alias`] reduces either to the
/// alias), and only an AWS-OFFICIAL name can name a lens here: a custom lens ARN carries
/// its owner's account id ([`is_aws_official_lens`]) and its content lives on its own
/// record in the store, not in this static catalog.
///
/// `None` for an official alias this catalog carries no content for — a lens AWS publishes
/// that has not been added here — in which case the optional members a lens review would
/// report about the lens are omitted rather than guessed at, and the lens asks no
/// questions here.
pub fn official_lens(lens: &str) -> Option<&'static OfficialLens> {
    if !is_aws_official_lens(lens) {
        return None;
    }
    let alias = lens_alias(lens);
    OFFICIAL_LENS_CATALOG
        .iter()
        .find(|official| official.alias == alias)
}

/// The questions a lens ATTACHED TO A WORKLOAD (or applied to a review template) asks, as
/// this emulator knows it, in the lens's own order.
///
/// An AWS-official lens asks what its catalog entry asks ([`official_lens`]). A CUSTOM
/// lens asks nothing HERE even once `CreateLensVersion` has published a version of it: its
/// question set is lens CONTENT, held on its own [`super::LensRecord`] in the store rather
/// than in this static catalog, and "the lens cannot be applied to workloads … until it's
/// published with CreateLensVersion" (`ImportLens` doc) is enforced by `AssociateLenses`,
/// still a stub with its own issue — so no custom lens in this emulator is attached to a
/// workload for its questions to be reported or counted.
pub fn lens_questions(lens: &str) -> &'static [FrameworkQuestion] {
    official_lens(lens).map_or(&[][..], |official| official.questions)
}

/// How many questions the named lens asks: the length of its [`lens_questions`], so the
/// roll-up and the answer list cannot disagree about the question set.
pub fn lens_question_count(lens: &str) -> usize {
    lens_questions(lens).len()
}

/// The pillars the named lens is organized into, in the framework's pillar order, each
/// with its `PillarName` and the number of questions that pillar asks — the pillar half of
/// [`lens_questions`].
///
/// Empty for a lens this emulator holds no content for (see [`lens_questions`]), which is
/// organized into no pillars HERE while the real lens is.
pub fn lens_pillars(lens: &str) -> Vec<(&'static str, &'static str, usize)> {
    official_lens(lens).map(OfficialLens::pillars).unwrap_or_default()
}

/// The named lens's own metadata, when this emulator holds that lens's content.
///
/// `None` for a lens it holds none for: a lens's name, version and status are lens
/// CONTENT, so for such a lens there is nothing to report and the optional members are
/// omitted rather than guessed at.
pub fn lens_metadata(lens: &str) -> Option<LensMetadata> {
    official_lens(lens).map(OfficialLens::metadata)
}

/// The AWS-official Well-Architected Framework lens's entry in this catalog: the first
/// row of [`OFFICIAL_LENS_CATALOG`], as [`FRAMEWORK_LENS`] states it once, so a caller
/// that wants THAT lens reaches it without a lookup that could miss.
pub fn framework_lens() -> &'static OfficialLens {
    &FRAMEWORK_LENS
}

/// When the framework lens version this emulator carries was published — midnight UTC of
/// the day [`super::FRAMEWORK_LENS_VERSION`] names.
pub fn framework_lens_published_at() -> SystemTime {
    framework_lens().published_at()
}

/// The AWS-official Well-Architected Framework lens, as this catalog carries it: the lens
/// [`FRAMEWORK_LENS_ALIAS`] names, at the question set this crate has always shipped for
/// it. Named so [`OFFICIAL_LENS_CATALOG`] and [`framework_lens`] cannot describe it
/// differently.
pub const FRAMEWORK_LENS: OfficialLens = OfficialLens {
    alias: FRAMEWORK_LENS_ALIAS,
    name: FRAMEWORK_LENS_NAME,
    version: FRAMEWORK_LENS_VERSION,
    description: FRAMEWORK_LENS_DESCRIPTION,
    questions: FRAMEWORK_LENS_QUESTIONS,
};

/// The AWS-official lenses this emulator publishes: the Well-Architected Framework lens
/// and the specialised lenses AWS publishes beside it.
///
/// The aliases and names are AWS's own (see this module's docs); the question sets are this
/// emulator's content for those lenses, at [`SPECIALISED_LENS_VERSION`].
pub const OFFICIAL_LENS_CATALOG: &[OfficialLens] = &[
    FRAMEWORK_LENS,
    OfficialLens {
        alias: "serverless",
        name: "Serverless Applications Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The Serverless Applications Lens covers how to design, deploy and \
             operate serverless application workloads: the health of an event-driven \
             application, access to its APIs, the request rates it regulates and the cost \
             of the functions and managed services it is built from.",
        questions: SERVERLESS_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "softwareasaservice",
        name: "SaaS Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The SaaS Lens covers how to design and operate a multi-tenant \
             software-as-a-service application: onboarding and offboarding tenants, \
             isolating tenant resources and data, keeping one tenant from affecting \
             another, and attributing consumption and cost to the tenant that caused it.",
        questions: SAAS_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "iot",
        name: "IoT Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The IoT Lens covers how to design and operate Internet of Things \
             workloads: provisioning device identity, authorizing what a device may \
             publish, ingesting and processing telemetry from a fleet, operating through \
             intermittent connectivity and delivering software updates to devices.",
        questions: IOT_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "machinelearning",
        name: "Machine Learning Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The Machine Learning Lens covers the machine learning lifecycle: \
             framing the business problem, preparing and versioning data, training and \
             tuning models, deploying them reproducibly, and monitoring the quality of the \
             predictions they make in production.",
        questions: MACHINE_LEARNING_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "dataanalytics",
        name: "Data Analytics Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The Data Analytics Lens covers how to design and operate analytics \
             workloads: ingesting data reliably, cataloging and governing it, choosing the \
             storage and query layer for the access pattern, and controlling what each \
             query and each stored data set costs.",
        questions: ANALYTICS_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "financialservices",
        name: "Financial Services Industry Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The Financial Services Industry Lens covers the obligations a \
             regulated financial institution's workloads carry: demonstrating compliance, \
             protecting customer and transaction data, keeping critical business services \
             available, and managing the third parties in their critical path.",
        questions: FINANCIAL_SERVICES_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "sap",
        name: "SAP Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The SAP Lens covers how to run an SAP landscape: sizing SAP systems \
             from SAP's own figures, keeping their central services and databases highly \
             available, backing them up and recovering them, and operating production and \
             non-production systems at a cost the business accepts.",
        questions: SAP_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "migration",
        name: "Migration Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The Migration Lens covers a migration to the cloud end to end: \
             assessing the portfolio and choosing a strategy per application, migrating in \
             waves, keeping data secure while it moves, validating and cutting over each \
             workload, and right-sizing it once it runs in its target environment.",
        questions: MIGRATION_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "genai",
        name: "Generative AI Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The Generative AI Lens covers how to build on foundation models: \
             selecting and sizing a model, grounding its responses in your own data, \
             evaluating the quality of what it generates, guarding prompts and responses, \
             and controlling the cost of inference.",
        questions: GENERATIVE_AI_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "healthcareindustry",
        name: "Healthcare Industry Lens",
        version: SPECIALISED_LENS_VERSION,
        description: "The Healthcare Industry Lens covers the obligations healthcare and \
             life sciences workloads carry: protecting patient health information, meeting \
             the regulations that govern its use, keeping clinical systems available to the \
             people delivering care, and exchanging clinical data through interoperability \
             standards.",
        questions: HEALTHCARE_LENS_QUESTIONS,
    },
];

/// The Serverless Applications Lens's question set: the lens's published focus areas, as
/// this emulator's content (see this module's docs), in pillar order.
const SERVERLESS_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "serverless-ops-01", "How do you evaluate your serverless application's health?", &[
        "Understand, analyze and alert on the metrics the platform emits",
        "Use distributed tracing across the application's components",
        "Collect, analyze and alert on application logs",
        "Instrument code with business and operational context",
    ]),
    q("operationalExcellence", "serverless-ops-02", "How do you approach application lifecycle management?", &[
        "Use infrastructure as code, with stages isolated in separate environments",
        "Prototype new features in temporary environments",
        "Use a staged rollout deployment mechanism",
        "Review the runtime deprecation policy of every function",
        "Manage configuration outside the deployment package",
    ]),
    q("security", "serverless-sec-01", "How do you control access to your serverless API?", &[
        "Choose the endpoint type and features that secure the API",
        "Authenticate and authorize every request to the API",
        "Scope access on the identity's own attributes",
        "Validate the request before it reaches business logic",
    ]),
    q("security", "serverless-sec-02", "How do you manage your serverless application's security boundaries?", &[
        "Define resource policies for every resource the application exposes",
        "Control network traffic at every layer the application spans",
        "Use temporary credentials between components",
        "Design smaller, single-purpose functions",
    ]),
    q("security", "serverless-sec-03", "How do you implement application security in your workload?", &[
        "Review code dependencies and libraries automatically",
        "Validate inbound events against a schema",
        "Store secrets in a secret store rather than in code or configuration",
        "Keep the team's security awareness current",
    ]),
    q("reliability", "serverless-rel-01", "How do you regulate inbound request rates?", &[
        "Throttle inbound requests to a rate the application can serve",
        "Publish, analyze and enforce API quotas per consumer",
        "Protect non-scalable downstream resources with a queue or a proxy",
        "Shed load deliberately rather than failing unpredictably",
    ]),
    q("reliability", "serverless-rel-02", "How do you build resiliency into your serverless application?", &[
        "Handle transaction, partial and intermittent failures",
        "Make event handling idempotent against duplicate deliveries",
        "Consider scaling behaviour at burst request rates",
        "Orchestrate long-running transactions explicitly",
    ]),
    q("performance", "serverless-perf-01", "How do you optimize your serverless application's performance?", &[
        "Measure, evaluate and select the optimal capacity unit",
        "Measure and optimize function startup time",
        "Optimize the runtime, the dependencies and the deployment package size",
        "Integrate with managed services directly rather than through a function",
        "Apply caching to repeated access patterns",
    ]),
    q("costOptimization", "serverless-cost-01", "How do you optimize your serverless application's costs?", &[
        "Minimize external calls and code initialization work",
        "Optimize logging output and its retention",
        "Tune function configuration to the work it does",
        "Use cost-aware usage patterns in code",
    ]),
];

/// The SaaS Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const SAAS_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "saas-ops-01", "How do you monitor and operate your multi-tenant environment?", &[
        "Aggregate metrics, logs and traces with the tenant context attached",
        "Alert on tenant-level health rather than only on fleet health",
        "Correlate an operational event with the tenant whose activity caused it",
        "Review tenant usage trends to guide operational change",
    ]),
    q("operationalExcellence", "saas-ops-02", "How do you onboard and offboard tenants?", &[
        "Automate tenant onboarding end to end",
        "Provision tenant resources from a template",
        "Record every tenant's configuration in a tenant management service",
        "Automate offboarding and the clean-up of a tenant's resources",
    ]),
    q("security", "saas-sec-01", "How do you isolate tenant resources and data?", &[
        "Apply an isolation boundary at every layer of the architecture",
        "Derive tenant-scoped credentials from the request's tenant context",
        "Test the isolation boundaries continuously",
        "Separate encryption keys per tenant where the tier requires it",
    ]),
    q("security", "saas-sec-02", "How do you authenticate and authorize tenants and their users?", &[
        "Authenticate tenant users through a centralized identity provider",
        "Inject the tenant context into every request",
        "Authorize a request against the tenant's tier and entitlements",
        "Audit administrative access to tenant data",
    ]),
    q("reliability", "saas-rel-01", "How do you prevent one tenant from affecting another?", &[
        "Apply per-tenant throttling and quotas",
        "Move a noisy workload into its own deployment",
        "Spread tenants across resources so one failure reaches few of them",
        "Degrade a tenant that exceeds its quota rather than the whole service",
    ]),
    q("performance", "saas-perf-01", "How do you scale your multi-tenant environment?", &[
        "Scale tenant tiers independently of one another",
        "Choose a data partitioning model per service",
        "Benchmark performance per tenant tier",
        "Cache data with the tenant context in the cache key",
    ]),
    q("costOptimization", "saas-cost-01", "How do you measure and attribute tenant cost?", &[
        "Tag resources with the tenant they serve",
        "Instrument tenant consumption of pooled resources",
        "Compare a tenant's cost against the tier it pays for",
        "Pool resources wherever the isolation model allows it",
    ]),
    q("sustainability", "saas-sus-01", "How do you improve the efficiency of your shared infrastructure?", &[
        "Pool tenant workloads to raise utilization",
        "Reclaim the resources of inactive tenants",
        "Right-size each tenant tier to its measured demand",
    ]),
];

/// The IoT Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const IOT_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "iot-ops-01", "How do you monitor the health of your device fleet?", &[
        "Collect device connectivity and lifecycle events",
        "Monitor message throughput and error rates per device",
        "Alert on fleet-wide anomalies rather than per device",
        "Record device state transitions for later analysis",
    ]),
    q("operationalExcellence", "iot-ops-02", "How do you deliver software and firmware updates to devices?", &[
        "Deploy updates in staged waves",
        "Verify the integrity of an update before applying it",
        "Roll back a device whose update failed",
        "Track the software version every device runs",
    ]),
    q("security", "iot-sec-01", "How do you provision device identity and credentials?", &[
        "Give every device a unique identity",
        "Provision credentials from a trusted certificate authority",
        "Rotate and revoke device credentials",
        "Keep credentials in hardware-protected storage on the device",
    ]),
    q("security", "iot-sec-02", "How do you authorize what a device may publish and subscribe to?", &[
        "Scope each device policy to the device's own identity",
        "Separate device topics by site or tenant",
        "Audit device authorization failures",
        "Quarantine a device that behaves anomalously",
    ]),
    q("reliability", "iot-rel-01", "How do you operate through intermittent device connectivity?", &[
        "Buffer telemetry on the device while it is offline",
        "Choose a quality-of-service level that matches the data's value",
        "Reconcile device state when connectivity returns",
        "Make message handling idempotent",
    ]),
    q("performance", "iot-perf-01", "How do you ingest and process device telemetry at scale?", &[
        "Batch and compress telemetry where latency allows",
        "Process at the edge what does not need the cloud",
        "Partition ingestion by device or site",
        "Choose a payload format sized for the network link",
    ]),
    q("costOptimization", "iot-cost-01", "How do you optimize the cost of device messaging and storage?", &[
        "Publish only the data the workload consumes",
        "Tier telemetry storage by the age of the data",
        "Aggregate measurements before transmitting them",
        "Review the per-message cost of each topic",
    ]),
    q("sustainability", "iot-sus-01", "How do you reduce the footprint of your device fleet?", &[
        "Extend device lifetime with maintainable firmware",
        "Transmit less data per measurement",
        "Schedule duty cycles to lower device power draw",
    ]),
];

/// The Machine Learning Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const MACHINE_LEARNING_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "ml-ops-01", "How do you frame the business problem your model solves?", &[
        "Define the business outcome the model serves",
        "Choose metrics that measure that outcome",
        "Establish a baseline before training a model",
        "Decide whether machine learning is the right approach at all",
    ]),
    q("operationalExcellence", "ml-ops-02", "How do you monitor models in production?", &[
        "Monitor inference quality against ground truth",
        "Detect data and concept drift",
        "Alert on inference latency and error rates",
        "Retrain on a trigger the monitoring provides",
    ]),
    q("security", "ml-sec-01", "How do you protect training data and model artifacts?", &[
        "Classify and encrypt training data",
        "Restrict who may read model artifacts and invoke endpoints",
        "Isolate training environments from production data stores",
        "Audit access to data sets and to models",
    ]),
    q("reliability", "ml-rel-01", "How do you make training and inference reproducible?", &[
        "Version data sets, code and model artifacts together",
        "Automate the training pipeline",
        "Record the lineage of every deployed model",
        "Keep a path back to the previous model version",
    ]),
    q("performance", "ml-perf-01", "How do you select and size compute for training and inference?", &[
        "Match the instance type to the model's compute profile",
        "Tune hyperparameters systematically rather than by hand",
        "Distribute training when the data set requires it",
        "Right-size inference endpoints to the request pattern",
    ]),
    q("costOptimization", "ml-cost-01", "How do you optimize the cost of training and inference?", &[
        "Use interruptible capacity for fault-tolerant training",
        "Stop idle notebooks and endpoints",
        "Use batch inference where latency allows",
        "Compress or distil models before deploying them",
    ]),
    q("sustainability", "ml-sus-01", "How do you limit the resources your models consume?", &[
        "Start from a pre-trained model where one fits",
        "Right-size the data set to what the model needs",
        "Retrain only when measured quality demands it",
    ]),
];

/// The Data Analytics Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const ANALYTICS_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "dataanalytics-ops-01", "How do you monitor and operate your analytics pipelines?", &[
        "Track the freshness and completeness of every data set",
        "Alert on failed and late data loads",
        "Record data quality metrics per data set",
        "Automate the recovery of a failed pipeline run",
    ]),
    q("operationalExcellence", "dataanalytics-ops-02", "How do you catalog your data so consumers can find it?", &[
        "Register every data set in a catalog",
        "Record ownership and schema for each data set",
        "Publish a data contract to the data set's consumers",
        "Track schema evolution over time",
    ]),
    q("security", "dataanalytics-sec-01", "How do you govern and protect sensitive data?", &[
        "Classify data sets by sensitivity",
        "Enforce fine-grained access to tables and columns",
        "Mask or tokenize sensitive attributes",
        "Audit who read which data set",
    ]),
    q("reliability", "dataanalytics-rel-01", "How do you make data ingestion reliable?", &[
        "Design ingestion so a load can be replayed",
        "Handle late and out-of-order data",
        "Validate data on arrival",
        "De-duplicate records idempotently",
    ]),
    q("performance", "dataanalytics-perf-01", "How do you choose the storage and query layer for your workload?", &[
        "Partition and compact data for the query pattern",
        "Use a columnar format for analytical scans",
        "Separate storage from compute where the workloads differ",
        "Benchmark queries against representative data volumes",
    ]),
    q("costOptimization", "dataanalytics-cost-01", "How do you optimize the cost of storing and querying data?", &[
        "Tier data by how often it is read",
        "Limit the data each query scans",
        "Right-size and pause idle compute",
        "Review the cost per query and per data set",
    ]),
    q("sustainability", "dataanalytics-sus-01", "How do you avoid storing and processing data nobody uses?", &[
        "Expire data nobody reads",
        "Remove duplicate copies of the same data set",
        "Process incrementally rather than reprocessing in full",
    ]),
];

/// The Financial Services Industry Lens's question set: the lens's published focus areas,
/// as this emulator's content (see this module's docs), in pillar order.
const FINANCIAL_SERVICES_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "fsi-ops-01", "How do you demonstrate compliance to your regulators?", &[
        "Record evidence of control operation continuously",
        "Map each control to the obligation it satisfies",
        "Automate the production of audit evidence",
        "Review the controls when the regulation changes",
    ]),
    q("security", "fsi-sec-01", "How do you protect customer and transaction data?", &[
        "Classify data by the obligation that governs it",
        "Encrypt data with keys the institution controls",
        "Restrict production data access to a named few",
        "Keep an immutable audit trail of data access",
    ]),
    q("security", "fsi-sec-02", "How do you manage third-party and vendor risk?", &[
        "Inventory every third party in the critical path",
        "Assess a vendor's controls before onboarding it",
        "Limit the data each vendor can reach",
        "Plan an exit from every vendor dependency",
    ]),
    q("reliability", "fsi-rel-01", "How do you keep critical business services available?", &[
        "Set recovery objectives per business service",
        "Test the failover of each critical service regularly",
        "Contain a failure to one business service",
        "Rehearse the operational response to an outage",
    ]),
    q("reliability", "fsi-rel-02", "How do you ensure the integrity of financial transactions?", &[
        "Make transaction processing idempotent",
        "Reconcile balances against an independent record",
        "Detect and quarantine suspect transactions",
        "Retain transaction history for the mandated period",
    ]),
    q("performance", "fsi-perf-01", "How do you meet the latency requirements of your workload?", &[
        "Measure latency at the percentiles the business cares about",
        "Place compute close to the market or payment endpoint",
        "Size capacity for peak market activity",
        "Remove variability from the critical path",
    ]),
    q("costOptimization", "fsi-cost-01", "How do you control the cost of regulated workloads?", &[
        "Attribute cost to the business service it serves",
        "Right-size the environments kept for audit and testing",
        "Review the cost of data retained for compliance",
        "Automate clean-up of resources a project no longer needs",
    ]),
];

/// The SAP Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const SAP_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "sap-ops-01", "How do you operate and monitor your SAP systems?", &[
        "Monitor the SAP application and its database together",
        "Integrate monitoring with SAP's own tooling",
        "Automate the start and stop of non-production systems",
        "Record every change to the SAP landscape",
    ]),
    q("security", "sap-sec-01", "How do you control access to your SAP landscape?", &[
        "Separate production from non-production access",
        "Administer SAP systems through a centralized identity",
        "Encrypt SAP data at rest and in transit",
        "Audit privileged activity in the SAP systems",
    ]),
    q("reliability", "sap-rel-01", "How do you make your SAP systems highly available?", &[
        "Deploy SAP central services as a highly available pair",
        "Replicate the SAP database across Availability Zones",
        "Test the failover of each SAP tier",
        "Define recovery objectives per SAP system",
    ]),
    q("reliability", "sap-rel-02", "How do you back up and recover your SAP systems?", &[
        "Back up SAP databases consistently",
        "Test the restore of a complete SAP system",
        "Retain backups for the business's retention period",
        "Protect backups from deletion",
    ]),
    q("performance", "sap-perf-01", "How do you size infrastructure for your SAP workload?", &[
        "Size instances from SAP's own benchmark figures",
        "Validate the sizing against the certified instance list",
        "Separate the database tier from the application tier",
        "Re-size as the SAP workload changes",
    ]),
    q("costOptimization", "sap-cost-01", "How do you optimize the cost of your SAP landscape?", &[
        "Shut down non-production systems outside business hours",
        "Commit to capacity for steady-state production systems",
        "Tier SAP backup storage by age",
        "Consolidate under-used SAP systems",
    ]),
    q("sustainability", "sap-sus-01", "How do you reduce the footprint of your SAP landscape?", &[
        "Archive SAP data nobody queries",
        "Retire SAP systems the business no longer uses",
        "Scale application servers to the business day",
    ]),
];

/// The Migration Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const MIGRATION_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "migration-ops-01", "How do you assess your portfolio before migrating?", &[
        "Inventory applications and their dependencies",
        "Choose a migration strategy per application",
        "Prioritize the portfolio by business value and risk",
        "Record the baseline each migration will be measured against",
    ]),
    q("operationalExcellence", "migration-ops-02", "How do you run the migration itself?", &[
        "Migrate in waves of related applications",
        "Automate the migration of repeatable workloads",
        "Define cut-over and rollback criteria per wave",
        "Capture lessons learned after each wave",
    ]),
    q("security", "migration-sec-01", "How do you keep data secure during migration?", &[
        "Encrypt data in transit to the target environment",
        "Limit who can reach migration tooling and credentials",
        "Verify that migrated data matches the source",
        "Decommission source systems securely",
    ]),
    q("reliability", "migration-rel-01", "How do you validate a migrated workload?", &[
        "Test the migrated workload against its baseline",
        "Rehearse the cut-over before the production run",
        "Keep a documented rollback path",
        "Verify backup and recovery in the target environment",
    ]),
    q("performance", "migration-perf-01", "How do you right-size workloads in the target environment?", &[
        "Size from measured utilization at the source",
        "Re-measure once the workload has stabilized",
        "Modernize components that cannot be sized efficiently",
    ]),
    q("costOptimization", "migration-cost-01", "How do you manage the cost of migrating and running the workload?", &[
        "Build a business case per migration wave",
        "Track migration cost against that business case",
        "Decommission source infrastructure promptly",
        "Apply pricing commitments once usage stabilizes",
    ]),
];

/// The Generative AI Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const GENERATIVE_AI_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "genai-ops-01", "How do you evaluate the quality of model output?", &[
        "Define quality metrics for each task the model performs",
        "Evaluate against a curated set of prompts",
        "Collect human feedback on responses",
        "Re-evaluate whenever the model or the prompt changes",
    ]),
    q("operationalExcellence", "genai-ops-02", "How do you monitor a generative AI application in production?", &[
        "Trace a response back to its prompt and the model version",
        "Monitor latency, token usage and error rates",
        "Alert on refusal and guardrail-intervention rates",
        "Review the responses the guardrails flagged",
    ]),
    q("security", "genai-sec-01", "How do you protect prompts, responses and the data behind them?", &[
        "Keep customer data out of model customization unless it was agreed",
        "Restrict who may invoke which model",
        "Scrub sensitive data from prompts and from logs",
        "Defend against prompt injection carried by untrusted content",
    ]),
    q("security", "genai-sec-02", "How do you apply responsible AI guardrails?", &[
        "Filter harmful content on input and on output",
        "Constrain the model to the application's domain",
        "Disclose to users that a response was generated by a model",
        "Keep a human in the loop for consequential decisions",
    ]),
    q("reliability", "genai-rel-01", "How do you ground model responses in your own data?", &[
        "Retrieve authoritative context before generating a response",
        "Keep the retrieval index synchronized with its source data",
        "Cite the source of a grounded response",
        "Fail closed when no context can be retrieved",
    ]),
    q("performance", "genai-perf-01", "How do you choose and size the model for your workload?", &[
        "Select the smallest model that meets the quality bar",
        "Stream responses where the interface allows it",
        "Cache responses to repeated prompts",
        "Benchmark candidate models on your own prompts",
    ]),
    q("costOptimization", "genai-cost-01", "How do you optimize inference cost?", &[
        "Measure cost per request and per token",
        "Shorten prompts and bound response length",
        "Route simple requests to cheaper models",
        "Batch inference that nobody is waiting for",
    ]),
    q("sustainability", "genai-sus-01", "How do you limit the resources inference and customization consume?", &[
        "Prefer customization that avoids training a model from scratch",
        "Reuse one foundation model across applications",
        "Retire experiments nobody uses",
    ]),
];

/// The Healthcare Industry Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const HEALTHCARE_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    q("operationalExcellence", "healthcare-ops-01", "How do you operate clinical systems without disrupting care?", &[
        "Schedule change around clinical workflows",
        "Monitor the clinical pathway end to end",
        "Rehearse the response to a clinical system outage",
        "Record the clinical impact of every incident",
    ]),
    q("security", "healthcare-sec-01", "How do you protect patient health information?", &[
        "Classify the data that identifies a patient",
        "Encrypt patient data with controlled keys",
        "Grant access to a patient record on a need-to-know basis",
        "Audit every access to a patient record",
    ]),
    q("security", "healthcare-sec-02", "How do you meet your healthcare regulatory obligations?", &[
        "Map each control to the regulation it satisfies",
        "Record consent and the purpose of each use of data",
        "Retain clinical records for the mandated period",
        "Review the obligations when entering a new jurisdiction",
    ]),
    q("reliability", "healthcare-rel-01", "How do you keep clinical workloads available?", &[
        "Set recovery objectives from the clinical risk",
        "Give clinicians a documented downtime procedure",
        "Replicate clinical data across Availability Zones",
        "Test the recovery of each clinical system",
    ]),
    q("performance", "healthcare-perf-01", "How do you exchange clinical data with other systems?", &[
        "Exchange data through healthcare interoperability standards",
        "Validate incoming clinical messages against the standard",
        "Size interfaces for peak clinical activity",
        "Reconcile patient identity across systems",
    ]),
    q("costOptimization", "healthcare-cost-01", "How do you manage the cost of clinical and research data?", &[
        "Tier imaging and genomic data by how often it is read",
        "Attribute cost to the clinical service or research study",
        "Right-size research compute to the analysis it runs",
        "Expire research data the study no longer needs",
    ]),
    q("sustainability", "healthcare-sus-01", "How do you limit the footprint of clinical and research data?", &[
        "Keep one authoritative copy of each clinical record",
        "Compress imaging and genomic data",
        "Run research analyses incrementally",
    ]),
];
