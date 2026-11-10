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
//! — and this catalog carries the CONTENT of those lenses: `ListLenses` reports a
//! `LensSummary` for each row here, so the published lens library a client discovers is
//! this table. A published alias with no row is a lens this emulator knows exists and
//! holds no questions for, which is therefore applicable but not listed, because a
//! `LensSummary` is content this emulator would have to invent. A test pins that
//! containment.
//!
//! The FRAMEWORK lens's content is the framework's own published content: its `question_id`s
//! and its choices' `ChoiceId`s and prose are the ones the Well-Architected Framework lens
//! publishes, carried verbatim (see [`super::FRAMEWORK_LENS_QUESTIONS`]).
//!
//! Below each SPECIALISED lens, the pillars, questions and best practices are a question
//! set written from that lens's published focus areas, under this emulator's own stable ids
//! ([`super::bp`]), because AWS publishes those lenses' content in no machine-readable form
//! the model or the service docs carry. A row whose published `LensVersion` is not one of
//! those AWS states for the lens reports [`AUTHORED_CONTENT_REVISION`] instead, which names
//! THIS catalog's revision of the question set it carries.
//!
//! ## Catalog METADATA is per lens
//!
//! A lens's description, the `LensVersion` its content is published at and the instants it
//! was created and last revised are that lens's OWN published data, so every row states its
//! own and nothing is shared across the catalog: a review of one lens cannot report
//! another's version, and a lens AWS has revised reports an `UpdatedAt` later than its
//! `CreatedAt` ([`OfficialLens::created_at`], [`OfficialLens::updated_at`]).
//!
//! The descriptions are the ones AWS's lens catalog publishes: the framework lens carries
//! its own published sentence ([`super::FRAMEWORK_LENS_DESCRIPTION`]) and a specialised lens
//! is described in the catalog's short form, `Best practices for <subject> workload` over the
//! subject its `LensName` names — the form the Connected Mobility lens is published with. A
//! test pins that form against each row's name, so a row cannot drift into prose of its own.
//!
//! The catalog is extensible by construction: adding a lens AWS publishes is adding a row
//! here and its alias to [`super::AWS_OFFICIAL_LENS_ALIASES`].

use std::time::{Duration, SystemTime};

use crate::ops::types::LensSummary;

use super::{
    bp, is_aws_official_lens, lens_alias, official_lens_arn, FrameworkQuestion,
    AWS_OFFICIAL_LENS_OWNER, FRAMEWORK_LENS_ALIAS, FRAMEWORK_LENS_DESCRIPTION,
    FRAMEWORK_LENS_NAME, FRAMEWORK_LENS_QUESTIONS, FRAMEWORK_LENS_VERSION,
    LENS_STATUS_CURRENT, LENS_STATUS_PUBLISHED, LENS_TYPE_AWS_OFFICIAL,
};

/// One question of a lens whose question set this emulator AUTHORED: the pillar, the
/// question id, the question and the titles of the best practices it asks about.
///
/// The counterpart of [`super::q`], which states a question of a lens that PUBLISHES its
/// content and therefore carries each choice's own id and prose: here each title becomes a
/// choice named by its position in the question and carrying no prose ([`bp`]), because
/// this emulator has no published id or prose to report for it. A macro rather than a
/// `const fn` because a slice of choices cannot be built from a slice of titles in a
/// constant — the expansion builds it one title at a time.
macro_rules! authored_question {
    ($pillar_id:expr, $question_id:expr, $title:expr, &[$($best_practice:expr),* $(,)?]) => {
        FrameworkQuestion {
            pillar_id: $pillar_id,
            question_id: $question_id,
            title: $title,
            best_practices: &[$(bp($best_practice)),*],
        }
    };
}

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

/// The `LensVersion` a catalog row reports when the version AWS publishes that lens's
/// content at is not one this emulator carries: the revision of this emulator's own content
/// for the lens.
///
/// It names THIS catalog's content, not a version string AWS publishes (the model publishes
/// none, and such a lens's question set here is this emulator's own) — the same reading as
/// [`super::FRAMEWORK_LENS_VERSION`], which names the revision of the framework lens's
/// question set. A change to that lens's questions changes it. `LensVersion` is
/// `@length(min: 1, max: 32)` (`smithy_query(query="shape", shape="LensReview")`), and a
/// date names a revision inside that.
pub const AUTHORED_CONTENT_REVISION: &str = "2025-01-15";

/// The form AWS's lens catalog describes a SPECIALISED published lens in — `Best practices
/// for <subject> workload`, the subject being what the lens's `LensName` names (the
/// Connected Mobility lens is published as `Best practices for Connected Mobility
/// workload`). Stated here as the two halves a row's `Description` is spelled from, so the
/// catalog and the test that pins the form read the same words.
pub const CATALOG_DESCRIPTION_PREFIX: &str = "Best practices for ";

/// The tail of [`CATALOG_DESCRIPTION_PREFIX`]'s form.
pub const CATALOG_DESCRIPTION_SUFFIX: &str = " workload";

/// The suffix a specialised lens's `LensName` ends in, which the catalog's description form
/// drops to name the lens's subject: `Connected Mobility Lens` is a lens about Connected
/// Mobility.
pub const LENS_NAME_SUFFIX: &str = " Lens";

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
    /// `LensSummary$LensVersion` / `LensReview$LensVersion`: the day AWS published the
    /// content this row carries, or [`AUTHORED_CONTENT_REVISION`] where this emulator's own
    /// content stands in for it. A date, so [`Self::updated_at`] can report the instant that
    /// version names.
    pub version: &'static str,
    /// The day AWS FIRST published this lens, for a lens it has since revised, and `None`
    /// for a lens this catalog carries at its first published version.
    ///
    /// `LensSummary$CreatedAt` is "The date and time when the lens was created", which for a
    /// revised lens is earlier than the day the version it now carries was published: the
    /// two instants are what tell a client the lens was ever revised, so a row that knows
    /// them apart states them apart ([`Self::created_at`]).
    pub first_published: Option<&'static str>,
    /// `LensSummary$Description` — the description AWS's lens catalog publishes for this
    /// lens, within `LensDescription`'s modelled 1..1024 characters
    /// (`smithy_query(query="shape", shape="LensSummary")`). A specialised lens is published
    /// in the catalog's short form (see this module's docs).
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
    /// `LensSummary$UpdatedAt` is "The date and time when the lens was last updated", and
    /// for a lens AWS publishes that is when the version it carries was published. Derived
    /// from the version rather than stored beside it, so the two cannot name different days,
    /// and read from the version rather than from the clock: a lens nobody changed must
    /// report the same instants to every caller.
    pub fn updated_at(&self) -> SystemTime {
        midnight_utc(self.version)
    }

    /// When this lens was created: midnight UTC of the day [`Self::first_published`] names,
    /// and of the version's own day for a lens that has never been revised.
    ///
    /// Never later than [`Self::updated_at`]: a lens is created before it is revised, which
    /// is what makes the pair readable as "this lens was revised after it was published".
    pub fn created_at(&self) -> SystemTime {
        midnight_utc(self.first_published.unwrap_or(self.version))
    }

    /// The `LensSummary` `ListLenses` reports this lens as.
    ///
    /// An AWS-official lens's ARN is partition-global and names the publisher rather than
    /// an owner account ([`official_lens_arn`]), so the summary carries the same identity
    /// in every Region, and `Owner` reports that publisher
    /// ([`AWS_OFFICIAL_LENS_OWNER`]): AWS publishes the lens rather than an account owning
    /// it, which is the distinction [`is_aws_official_lens`] reads.
    pub fn summary(&self) -> LensSummary {
        LensSummary {
            lens_arn: Some(official_lens_arn(self.alias)),
            lens_alias: Some(self.alias.to_string()),
            lens_name: Some(self.name.to_string()),
            lens_type: Some(LENS_TYPE_AWS_OFFICIAL.to_string()),
            description: Some(self.description.to_string()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at())),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at())),
            lens_version: Some(self.version.to_string()),
            owner: Some(AWS_OFFICIAL_LENS_OWNER.to_string()),
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
///
/// Shared with the other published content whose revision is named by a day — the profile
/// template's ([`super::profile_template::PROFILE_TEMPLATE_REVISION`]) — so one place
/// decides what instant a published revision reports.
pub(super) fn midnight_utc(day: &str) -> SystemTime {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        &format!("{day}T00:00:00Z"),
        ::aws_smithy_types::date_time::Format::DateTime,
    );
    debug_assert!(
        parsed.is_ok(),
        "a published revision must name a calendar day: {day:?}"
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

/// The questions an AWS-OFFICIAL lens asks, as this emulator's catalog carries them, in
/// the lens's own order.
///
/// The official half of a lens review's question set: a lens AWS publishes asks what its
/// catalog entry asks ([`official_lens`]), and empty for an official alias this catalog
/// carries no content for. A CUSTOM lens's questions are not here — its question set is
/// lens CONTENT, held on its own [`super::LensRecord`] in the store — so an operation that
/// reports the questions of whatever lens a request named resolves them through
/// [`super::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_review_questions`],
/// which is this table for an official lens and the imported document for a custom one.
pub fn lens_questions(lens: &str) -> &'static [FrameworkQuestion] {
    official_lens(lens).map_or(&[][..], |official| official.questions)
}

/// How many questions the named lens asks: the length of its [`lens_questions`], so the
/// roll-up and the answer list cannot disagree about the question set.
pub fn lens_question_count(lens: &str) -> usize {
    lens_questions(lens).len()
}

/// The ONE question an AWS-official lens asks under `question_id`, when it asks it.
///
/// `QuestionId` is how every operation that reads or writes a single answer names the
/// question, so this is the one lookup over this catalog they share — the official-lens
/// counterpart of
/// [`super::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::lens_review_question`],
/// which resolves a question of WHATEVER lens a request named.
pub fn lens_question(lens: &str, question_id: &str) -> Option<&'static FrameworkQuestion> {
    lens_questions(lens)
        .iter()
        .find(|question| question.question_id == question_id)
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
/// the day [`super::FRAMEWORK_LENS_VERSION`] names, which is the instant that lens's
/// summary reports as its `UpdatedAt`.
pub fn framework_lens_published_at() -> SystemTime {
    framework_lens().updated_at()
}

/// The AWS-official Well-Architected Framework lens, as this catalog carries it: the lens
/// [`FRAMEWORK_LENS_ALIAS`] names, at the question set this crate has always shipped for
/// it. Named so [`OFFICIAL_LENS_CATALOG`] and [`framework_lens`] cannot describe it
/// differently.
pub const FRAMEWORK_LENS: OfficialLens = OfficialLens {
    alias: FRAMEWORK_LENS_ALIAS,
    name: FRAMEWORK_LENS_NAME,
    version: FRAMEWORK_LENS_VERSION,
    // The framework lens is the one lens in the catalog AWS has published REVISIONS of: the
    // day AWS's lens catalog reports it was created, which is well before the day the
    // version it carries now was published.
    first_published: Some("2019-09-13"),
    description: FRAMEWORK_LENS_DESCRIPTION,
    questions: FRAMEWORK_LENS_QUESTIONS,
};

/// The AWS-official lenses this emulator publishes: the Well-Architected Framework lens
/// and the specialised lenses AWS publishes beside it.
///
/// The aliases, names, descriptions and — where AWS states one for the lens — the versions
/// are AWS's own (see this module's docs); a row carrying no version of AWS's reports
/// [`AUTHORED_CONTENT_REVISION`], the revision of this emulator's question set for it.
pub const OFFICIAL_LENS_CATALOG: &[OfficialLens] = &[
    FRAMEWORK_LENS,
    OfficialLens {
        alias: "serverless",
        name: "Serverless Lens",
        // The day AWS publishes the Serverless lens at.
        version: "2020-02-04",
        first_published: None,
        description: "Best practices for Serverless workload",
        questions: SERVERLESS_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "softwareasaservice",
        name: "SaaS Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for SaaS workload",
        questions: SAAS_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "iot",
        name: "IoT Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for IoT workload",
        questions: IOT_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "machinelearning",
        name: "Machine Learning Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for Machine Learning workload",
        questions: MACHINE_LEARNING_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "dataanalytics",
        name: "Data Analytics Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for Data Analytics workload",
        questions: ANALYTICS_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "financialservices",
        name: "Financial Services Industry Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for Financial Services Industry workload",
        questions: FINANCIAL_SERVICES_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "sap",
        name: "SAP Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for SAP workload",
        questions: SAP_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "migration",
        name: "Migration Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for Migration workload",
        questions: MIGRATION_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "genai",
        name: "Generative AI Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for Generative AI workload",
        questions: GENERATIVE_AI_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "healthcareindustry",
        name: "Healthcare Industry Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for Healthcare Industry workload",
        questions: HEALTHCARE_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "connectedmobility",
        name: "Connected Mobility Lens",
        // The day AWS publishes the Connected Mobility lens at.
        version: "2024-03-26",
        first_published: None,
        description: "Best practices for Connected Mobility workload",
        questions: CONNECTED_MOBILITY_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "devops",
        name: "DevOps Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for DevOps workload",
        questions: DEVOPS_LENS_QUESTIONS,
    },
    OfficialLens {
        alias: "games",
        name: "Games Industry Lens",
        version: AUTHORED_CONTENT_REVISION,
        first_published: None,
        description: "Best practices for Games Industry workload",
        questions: GAMES_LENS_QUESTIONS,
    },
];

/// The Serverless Applications Lens's question set: the lens's published focus areas, as
/// this emulator's content (see this module's docs), in pillar order.
const SERVERLESS_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "serverless-ops-01", "How do you evaluate your serverless application's health?", &[
        "Understand, analyze and alert on the metrics the platform emits",
        "Use distributed tracing across the application's components",
        "Collect, analyze and alert on application logs",
        "Instrument code with business and operational context",
    ]),
    authored_question!("operationalExcellence", "serverless-ops-02", "How do you approach application lifecycle management?", &[
        "Use infrastructure as code, with stages isolated in separate environments",
        "Prototype new features in temporary environments",
        "Use a staged rollout deployment mechanism",
        "Review the runtime deprecation policy of every function",
        "Manage configuration outside the deployment package",
    ]),
    authored_question!("security", "serverless-sec-01", "How do you control access to your serverless API?", &[
        "Choose the endpoint type and features that secure the API",
        "Authenticate and authorize every request to the API",
        "Scope access on the identity's own attributes",
        "Validate the request before it reaches business logic",
    ]),
    authored_question!("security", "serverless-sec-02", "How do you manage your serverless application's security boundaries?", &[
        "Define resource policies for every resource the application exposes",
        "Control network traffic at every layer the application spans",
        "Use temporary credentials between components",
        "Design smaller, single-purpose functions",
    ]),
    authored_question!("security", "serverless-sec-03", "How do you implement application security in your workload?", &[
        "Review code dependencies and libraries automatically",
        "Validate inbound events against a schema",
        "Store secrets in a secret store rather than in code or configuration",
        "Keep the team's security awareness current",
    ]),
    authored_question!("reliability", "serverless-rel-01", "How do you regulate inbound request rates?", &[
        "Throttle inbound requests to a rate the application can serve",
        "Publish, analyze and enforce API quotas per consumer",
        "Protect non-scalable downstream resources with a queue or a proxy",
        "Shed load deliberately rather than failing unpredictably",
    ]),
    authored_question!("reliability", "serverless-rel-02", "How do you build resiliency into your serverless application?", &[
        "Handle transaction, partial and intermittent failures",
        "Make event handling idempotent against duplicate deliveries",
        "Consider scaling behaviour at burst request rates",
        "Orchestrate long-running transactions explicitly",
    ]),
    authored_question!("performance", "serverless-perf-01", "How do you optimize your serverless application's performance?", &[
        "Measure, evaluate and select the optimal capacity unit",
        "Measure and optimize function startup time",
        "Optimize the runtime, the dependencies and the deployment package size",
        "Integrate with managed services directly rather than through a function",
        "Apply caching to repeated access patterns",
    ]),
    authored_question!("costOptimization", "serverless-cost-01", "How do you optimize your serverless application's costs?", &[
        "Minimize external calls and code initialization work",
        "Optimize logging output and its retention",
        "Tune function configuration to the work it does",
        "Use cost-aware usage patterns in code",
    ]),
];

/// The SaaS Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const SAAS_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "saas-ops-01", "How do you monitor and operate your multi-tenant environment?", &[
        "Aggregate metrics, logs and traces with the tenant context attached",
        "Alert on tenant-level health rather than only on fleet health",
        "Correlate an operational event with the tenant whose activity caused it",
        "Review tenant usage trends to guide operational change",
    ]),
    authored_question!("operationalExcellence", "saas-ops-02", "How do you onboard and offboard tenants?", &[
        "Automate tenant onboarding end to end",
        "Provision tenant resources from a template",
        "Record every tenant's configuration in a tenant management service",
        "Automate offboarding and the clean-up of a tenant's resources",
    ]),
    authored_question!("security", "saas-sec-01", "How do you isolate tenant resources and data?", &[
        "Apply an isolation boundary at every layer of the architecture",
        "Derive tenant-scoped credentials from the request's tenant context",
        "Test the isolation boundaries continuously",
        "Separate encryption keys per tenant where the tier requires it",
    ]),
    authored_question!("security", "saas-sec-02", "How do you authenticate and authorize tenants and their users?", &[
        "Authenticate tenant users through a centralized identity provider",
        "Inject the tenant context into every request",
        "Authorize a request against the tenant's tier and entitlements",
        "Audit administrative access to tenant data",
    ]),
    authored_question!("reliability", "saas-rel-01", "How do you prevent one tenant from affecting another?", &[
        "Apply per-tenant throttling and quotas",
        "Move a noisy workload into its own deployment",
        "Spread tenants across resources so one failure reaches few of them",
        "Degrade a tenant that exceeds its quota rather than the whole service",
    ]),
    authored_question!("performance", "saas-perf-01", "How do you scale your multi-tenant environment?", &[
        "Scale tenant tiers independently of one another",
        "Choose a data partitioning model per service",
        "Benchmark performance per tenant tier",
        "Cache data with the tenant context in the cache key",
    ]),
    authored_question!("costOptimization", "saas-cost-01", "How do you measure and attribute tenant cost?", &[
        "Tag resources with the tenant they serve",
        "Instrument tenant consumption of pooled resources",
        "Compare a tenant's cost against the tier it pays for",
        "Pool resources wherever the isolation model allows it",
    ]),
    authored_question!("sustainability", "saas-sus-01", "How do you improve the efficiency of your shared infrastructure?", &[
        "Pool tenant workloads to raise utilization",
        "Reclaim the resources of inactive tenants",
        "Right-size each tenant tier to its measured demand",
    ]),
];

/// The IoT Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const IOT_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "iot-ops-01", "How do you monitor the health of your device fleet?", &[
        "Collect device connectivity and lifecycle events",
        "Monitor message throughput and error rates per device",
        "Alert on fleet-wide anomalies rather than per device",
        "Record device state transitions for later analysis",
    ]),
    authored_question!("operationalExcellence", "iot-ops-02", "How do you deliver software and firmware updates to devices?", &[
        "Deploy updates in staged waves",
        "Verify the integrity of an update before applying it",
        "Roll back a device whose update failed",
        "Track the software version every device runs",
    ]),
    authored_question!("security", "iot-sec-01", "How do you provision device identity and credentials?", &[
        "Give every device a unique identity",
        "Provision credentials from a trusted certificate authority",
        "Rotate and revoke device credentials",
        "Keep credentials in hardware-protected storage on the device",
    ]),
    authored_question!("security", "iot-sec-02", "How do you authorize what a device may publish and subscribe to?", &[
        "Scope each device policy to the device's own identity",
        "Separate device topics by site or tenant",
        "Audit device authorization failures",
        "Quarantine a device that behaves anomalously",
    ]),
    authored_question!("reliability", "iot-rel-01", "How do you operate through intermittent device connectivity?", &[
        "Buffer telemetry on the device while it is offline",
        "Choose a quality-of-service level that matches the data's value",
        "Reconcile device state when connectivity returns",
        "Make message handling idempotent",
    ]),
    authored_question!("performance", "iot-perf-01", "How do you ingest and process device telemetry at scale?", &[
        "Batch and compress telemetry where latency allows",
        "Process at the edge what does not need the cloud",
        "Partition ingestion by device or site",
        "Choose a payload format sized for the network link",
    ]),
    authored_question!("costOptimization", "iot-cost-01", "How do you optimize the cost of device messaging and storage?", &[
        "Publish only the data the workload consumes",
        "Tier telemetry storage by the age of the data",
        "Aggregate measurements before transmitting them",
        "Review the per-message cost of each topic",
    ]),
    authored_question!("sustainability", "iot-sus-01", "How do you reduce the footprint of your device fleet?", &[
        "Extend device lifetime with maintainable firmware",
        "Transmit less data per measurement",
        "Schedule duty cycles to lower device power draw",
    ]),
];

/// The Machine Learning Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const MACHINE_LEARNING_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "ml-ops-01", "How do you frame the business problem your model solves?", &[
        "Define the business outcome the model serves",
        "Choose metrics that measure that outcome",
        "Establish a baseline before training a model",
        "Decide whether machine learning is the right approach at all",
    ]),
    authored_question!("operationalExcellence", "ml-ops-02", "How do you monitor models in production?", &[
        "Monitor inference quality against ground truth",
        "Detect data and concept drift",
        "Alert on inference latency and error rates",
        "Retrain on a trigger the monitoring provides",
    ]),
    authored_question!("security", "ml-sec-01", "How do you protect training data and model artifacts?", &[
        "Classify and encrypt training data",
        "Restrict who may read model artifacts and invoke endpoints",
        "Isolate training environments from production data stores",
        "Audit access to data sets and to models",
    ]),
    authored_question!("reliability", "ml-rel-01", "How do you make training and inference reproducible?", &[
        "Version data sets, code and model artifacts together",
        "Automate the training pipeline",
        "Record the lineage of every deployed model",
        "Keep a path back to the previous model version",
    ]),
    authored_question!("performance", "ml-perf-01", "How do you select and size compute for training and inference?", &[
        "Match the instance type to the model's compute profile",
        "Tune hyperparameters systematically rather than by hand",
        "Distribute training when the data set requires it",
        "Right-size inference endpoints to the request pattern",
    ]),
    authored_question!("costOptimization", "ml-cost-01", "How do you optimize the cost of training and inference?", &[
        "Use interruptible capacity for fault-tolerant training",
        "Stop idle notebooks and endpoints",
        "Use batch inference where latency allows",
        "Compress or distil models before deploying them",
    ]),
    authored_question!("sustainability", "ml-sus-01", "How do you limit the resources your models consume?", &[
        "Start from a pre-trained model where one fits",
        "Right-size the data set to what the model needs",
        "Retrain only when measured quality demands it",
    ]),
];

/// The Data Analytics Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const ANALYTICS_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "dataanalytics-ops-01", "How do you monitor and operate your analytics pipelines?", &[
        "Track the freshness and completeness of every data set",
        "Alert on failed and late data loads",
        "Record data quality metrics per data set",
        "Automate the recovery of a failed pipeline run",
    ]),
    authored_question!("operationalExcellence", "dataanalytics-ops-02", "How do you catalog your data so consumers can find it?", &[
        "Register every data set in a catalog",
        "Record ownership and schema for each data set",
        "Publish a data contract to the data set's consumers",
        "Track schema evolution over time",
    ]),
    authored_question!("security", "dataanalytics-sec-01", "How do you govern and protect sensitive data?", &[
        "Classify data sets by sensitivity",
        "Enforce fine-grained access to tables and columns",
        "Mask or tokenize sensitive attributes",
        "Audit who read which data set",
    ]),
    authored_question!("reliability", "dataanalytics-rel-01", "How do you make data ingestion reliable?", &[
        "Design ingestion so a load can be replayed",
        "Handle late and out-of-order data",
        "Validate data on arrival",
        "De-duplicate records idempotently",
    ]),
    authored_question!("performance", "dataanalytics-perf-01", "How do you choose the storage and query layer for your workload?", &[
        "Partition and compact data for the query pattern",
        "Use a columnar format for analytical scans",
        "Separate storage from compute where the workloads differ",
        "Benchmark queries against representative data volumes",
    ]),
    authored_question!("costOptimization", "dataanalytics-cost-01", "How do you optimize the cost of storing and querying data?", &[
        "Tier data by how often it is read",
        "Limit the data each query scans",
        "Right-size and pause idle compute",
        "Review the cost per query and per data set",
    ]),
    authored_question!("sustainability", "dataanalytics-sus-01", "How do you avoid storing and processing data nobody uses?", &[
        "Expire data nobody reads",
        "Remove duplicate copies of the same data set",
        "Process incrementally rather than reprocessing in full",
    ]),
];

/// The Financial Services Industry Lens's question set: the lens's published focus areas,
/// as this emulator's content (see this module's docs), in pillar order.
const FINANCIAL_SERVICES_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "fsi-ops-01", "How do you demonstrate compliance to your regulators?", &[
        "Record evidence of control operation continuously",
        "Map each control to the obligation it satisfies",
        "Automate the production of audit evidence",
        "Review the controls when the regulation changes",
    ]),
    authored_question!("security", "fsi-sec-01", "How do you protect customer and transaction data?", &[
        "Classify data by the obligation that governs it",
        "Encrypt data with keys the institution controls",
        "Restrict production data access to a named few",
        "Keep an immutable audit trail of data access",
    ]),
    authored_question!("security", "fsi-sec-02", "How do you manage third-party and vendor risk?", &[
        "Inventory every third party in the critical path",
        "Assess a vendor's controls before onboarding it",
        "Limit the data each vendor can reach",
        "Plan an exit from every vendor dependency",
    ]),
    authored_question!("reliability", "fsi-rel-01", "How do you keep critical business services available?", &[
        "Set recovery objectives per business service",
        "Test the failover of each critical service regularly",
        "Contain a failure to one business service",
        "Rehearse the operational response to an outage",
    ]),
    authored_question!("reliability", "fsi-rel-02", "How do you ensure the integrity of financial transactions?", &[
        "Make transaction processing idempotent",
        "Reconcile balances against an independent record",
        "Detect and quarantine suspect transactions",
        "Retain transaction history for the mandated period",
    ]),
    authored_question!("performance", "fsi-perf-01", "How do you meet the latency requirements of your workload?", &[
        "Measure latency at the percentiles the business cares about",
        "Place compute close to the market or payment endpoint",
        "Size capacity for peak market activity",
        "Remove variability from the critical path",
    ]),
    authored_question!("costOptimization", "fsi-cost-01", "How do you control the cost of regulated workloads?", &[
        "Attribute cost to the business service it serves",
        "Right-size the environments kept for audit and testing",
        "Review the cost of data retained for compliance",
        "Automate clean-up of resources a project no longer needs",
    ]),
];

/// The SAP Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const SAP_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "sap-ops-01", "How do you operate and monitor your SAP systems?", &[
        "Monitor the SAP application and its database together",
        "Integrate monitoring with SAP's own tooling",
        "Automate the start and stop of non-production systems",
        "Record every change to the SAP landscape",
    ]),
    authored_question!("security", "sap-sec-01", "How do you control access to your SAP landscape?", &[
        "Separate production from non-production access",
        "Administer SAP systems through a centralized identity",
        "Encrypt SAP data at rest and in transit",
        "Audit privileged activity in the SAP systems",
    ]),
    authored_question!("reliability", "sap-rel-01", "How do you make your SAP systems highly available?", &[
        "Deploy SAP central services as a highly available pair",
        "Replicate the SAP database across Availability Zones",
        "Test the failover of each SAP tier",
        "Define recovery objectives per SAP system",
    ]),
    authored_question!("reliability", "sap-rel-02", "How do you back up and recover your SAP systems?", &[
        "Back up SAP databases consistently",
        "Test the restore of a complete SAP system",
        "Retain backups for the business's retention period",
        "Protect backups from deletion",
    ]),
    authored_question!("performance", "sap-perf-01", "How do you size infrastructure for your SAP workload?", &[
        "Size instances from SAP's own benchmark figures",
        "Validate the sizing against the certified instance list",
        "Separate the database tier from the application tier",
        "Re-size as the SAP workload changes",
    ]),
    authored_question!("costOptimization", "sap-cost-01", "How do you optimize the cost of your SAP landscape?", &[
        "Shut down non-production systems outside business hours",
        "Commit to capacity for steady-state production systems",
        "Tier SAP backup storage by age",
        "Consolidate under-used SAP systems",
    ]),
    authored_question!("sustainability", "sap-sus-01", "How do you reduce the footprint of your SAP landscape?", &[
        "Archive SAP data nobody queries",
        "Retire SAP systems the business no longer uses",
        "Scale application servers to the business day",
    ]),
];

/// The Migration Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const MIGRATION_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "migration-ops-01", "How do you assess your portfolio before migrating?", &[
        "Inventory applications and their dependencies",
        "Choose a migration strategy per application",
        "Prioritize the portfolio by business value and risk",
        "Record the baseline each migration will be measured against",
    ]),
    authored_question!("operationalExcellence", "migration-ops-02", "How do you run the migration itself?", &[
        "Migrate in waves of related applications",
        "Automate the migration of repeatable workloads",
        "Define cut-over and rollback criteria per wave",
        "Capture lessons learned after each wave",
    ]),
    authored_question!("security", "migration-sec-01", "How do you keep data secure during migration?", &[
        "Encrypt data in transit to the target environment",
        "Limit who can reach migration tooling and credentials",
        "Verify that migrated data matches the source",
        "Decommission source systems securely",
    ]),
    authored_question!("reliability", "migration-rel-01", "How do you validate a migrated workload?", &[
        "Test the migrated workload against its baseline",
        "Rehearse the cut-over before the production run",
        "Keep a documented rollback path",
        "Verify backup and recovery in the target environment",
    ]),
    authored_question!("performance", "migration-perf-01", "How do you right-size workloads in the target environment?", &[
        "Size from measured utilization at the source",
        "Re-measure once the workload has stabilized",
        "Modernize components that cannot be sized efficiently",
    ]),
    authored_question!("costOptimization", "migration-cost-01", "How do you manage the cost of migrating and running the workload?", &[
        "Build a business case per migration wave",
        "Track migration cost against that business case",
        "Decommission source infrastructure promptly",
        "Apply pricing commitments once usage stabilizes",
    ]),
];

/// The Generative AI Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const GENERATIVE_AI_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "genai-ops-01", "How do you evaluate the quality of model output?", &[
        "Define quality metrics for each task the model performs",
        "Evaluate against a curated set of prompts",
        "Collect human feedback on responses",
        "Re-evaluate whenever the model or the prompt changes",
    ]),
    authored_question!("operationalExcellence", "genai-ops-02", "How do you monitor a generative AI application in production?", &[
        "Trace a response back to its prompt and the model version",
        "Monitor latency, token usage and error rates",
        "Alert on refusal and guardrail-intervention rates",
        "Review the responses the guardrails flagged",
    ]),
    authored_question!("security", "genai-sec-01", "How do you protect prompts, responses and the data behind them?", &[
        "Keep customer data out of model customization unless it was agreed",
        "Restrict who may invoke which model",
        "Scrub sensitive data from prompts and from logs",
        "Defend against prompt injection carried by untrusted content",
    ]),
    authored_question!("security", "genai-sec-02", "How do you apply responsible AI guardrails?", &[
        "Filter harmful content on input and on output",
        "Constrain the model to the application's domain",
        "Disclose to users that a response was generated by a model",
        "Keep a human in the loop for consequential decisions",
    ]),
    authored_question!("reliability", "genai-rel-01", "How do you ground model responses in your own data?", &[
        "Retrieve authoritative context before generating a response",
        "Keep the retrieval index synchronized with its source data",
        "Cite the source of a grounded response",
        "Fail closed when no context can be retrieved",
    ]),
    authored_question!("performance", "genai-perf-01", "How do you choose and size the model for your workload?", &[
        "Select the smallest model that meets the quality bar",
        "Stream responses where the interface allows it",
        "Cache responses to repeated prompts",
        "Benchmark candidate models on your own prompts",
    ]),
    authored_question!("costOptimization", "genai-cost-01", "How do you optimize inference cost?", &[
        "Measure cost per request and per token",
        "Shorten prompts and bound response length",
        "Route simple requests to cheaper models",
        "Batch inference that nobody is waiting for",
    ]),
    authored_question!("sustainability", "genai-sus-01", "How do you limit the resources inference and customization consume?", &[
        "Prefer customization that avoids training a model from scratch",
        "Reuse one foundation model across applications",
        "Retire experiments nobody uses",
    ]),
];

/// The Healthcare Industry Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const HEALTHCARE_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "healthcare-ops-01", "How do you operate clinical systems without disrupting care?", &[
        "Schedule change around clinical workflows",
        "Monitor the clinical pathway end to end",
        "Rehearse the response to a clinical system outage",
        "Record the clinical impact of every incident",
    ]),
    authored_question!("security", "healthcare-sec-01", "How do you protect patient health information?", &[
        "Classify the data that identifies a patient",
        "Encrypt patient data with controlled keys",
        "Grant access to a patient record on a need-to-know basis",
        "Audit every access to a patient record",
    ]),
    authored_question!("security", "healthcare-sec-02", "How do you meet your healthcare regulatory obligations?", &[
        "Map each control to the regulation it satisfies",
        "Record consent and the purpose of each use of data",
        "Retain clinical records for the mandated period",
        "Review the obligations when entering a new jurisdiction",
    ]),
    authored_question!("reliability", "healthcare-rel-01", "How do you keep clinical workloads available?", &[
        "Set recovery objectives from the clinical risk",
        "Give clinicians a documented downtime procedure",
        "Replicate clinical data across Availability Zones",
        "Test the recovery of each clinical system",
    ]),
    authored_question!("performance", "healthcare-perf-01", "How do you exchange clinical data with other systems?", &[
        "Exchange data through healthcare interoperability standards",
        "Validate incoming clinical messages against the standard",
        "Size interfaces for peak clinical activity",
        "Reconcile patient identity across systems",
    ]),
    authored_question!("costOptimization", "healthcare-cost-01", "How do you manage the cost of clinical and research data?", &[
        "Tier imaging and genomic data by how often it is read",
        "Attribute cost to the clinical service or research study",
        "Right-size research compute to the analysis it runs",
        "Expire research data the study no longer needs",
    ]),
    authored_question!("sustainability", "healthcare-sus-01", "How do you limit the footprint of clinical and research data?", &[
        "Keep one authoritative copy of each clinical record",
        "Compress imaging and genomic data",
        "Run research analyses incrementally",
    ]),
];

/// The Connected Mobility Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const CONNECTED_MOBILITY_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "connectedmobility-ops-01", "How do you operate a connected vehicle fleet?", &[
        "Monitor vehicle connectivity and session health",
        "Alert on fleet-wide anomalies rather than per vehicle",
        "Record the software and hardware configuration of every vehicle",
        "Rehearse the response to a fleet-wide incident",
    ]),
    authored_question!("operationalExcellence", "connectedmobility-ops-02", "How do you deliver software to vehicles over the air?", &[
        "Deploy vehicle software in staged campaigns",
        "Verify the integrity of an update before a vehicle applies it",
        "Give every campaign a rollback path",
        "Apply updates only when the vehicle is in a safe state",
    ]),
    authored_question!("security", "connectedmobility-sec-01", "How do you establish and protect vehicle identity?", &[
        "Provision a unique identity per vehicle at manufacture",
        "Keep vehicle credentials in hardware-protected storage",
        "Rotate and revoke the credentials of a vehicle",
        "Authorize each vehicle to its own topics and APIs",
    ]),
    authored_question!("security", "connectedmobility-sec-02", "How do you protect the personal data a vehicle produces?", &[
        "Classify vehicle data by whether it identifies a person",
        "Record the driver's consent for each use of data",
        "Minimize the personal data leaving the vehicle",
        "Honour deletion requests across every store",
    ]),
    authored_question!("reliability", "connectedmobility-rel-01", "How do you operate through intermittent vehicle connectivity?", &[
        "Buffer vehicle data while the vehicle is offline",
        "Reconcile vehicle state when connectivity returns",
        "Make the handling of vehicle messages idempotent",
        "Degrade in-vehicle features rather than failing them",
    ]),
    authored_question!("performance", "connectedmobility-perf-01", "How do you ingest and process vehicle telemetry at scale?", &[
        "Partition ingestion by vehicle, model or region",
        "Process at the edge what does not need the cloud",
        "Choose a payload format sized for the mobile link",
        "Benchmark ingestion against a launch-sized fleet",
    ]),
    authored_question!("costOptimization", "connectedmobility-cost-01", "How do you control the cost of a connected fleet?", &[
        "Publish only the signals a workload consumes",
        "Tier vehicle data storage by the age of the data",
        "Review the per-vehicle cost of connectivity and storage",
        "Aggregate signals in the vehicle before transmitting them",
    ]),
    authored_question!("sustainability", "connectedmobility-sus-01", "How do you reduce the footprint of your mobility workloads?", &[
        "Transmit less data per journey",
        "Extend vehicle lifetime with maintainable software",
        "Expire telemetry nobody analyses",
    ]),
];

/// The DevOps Lens's question set: the lens's published focus areas, as this emulator's
/// content (see this module's docs), in pillar order.
const DEVOPS_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "devops-ops-01", "How do you organize your teams to deliver and operate software?", &[
        "Give every service a team that owns it end to end",
        "Make the ownership of each component discoverable",
        "Agree the interfaces and expectations between teams",
        "Measure delivery with team-level metrics",
    ]),
    authored_question!("operationalExcellence", "devops-ops-02", "How do you automate the path from commit to production?", &[
        "Build and test every commit automatically",
        "Promote one artifact through every environment",
        "Gate promotion on automated quality checks",
        "Keep the pipeline itself in version control",
    ]),
    authored_question!("operationalExcellence", "devops-ops-03", "How do you learn from what you release?", &[
        "Trace a production signal back to the change that caused it",
        "Review incidents without blame and act on the findings",
        "Feed operational findings into the delivery backlog",
        "Share what one team learns with the others",
    ]),
    authored_question!("security", "devops-sec-01", "How do you secure your delivery pipeline?", &[
        "Scope the pipeline's permissions to what it deploys",
        "Scan dependencies and images on every build",
        "Sign artifacts and verify them before deployment",
        "Keep deployment secrets in a secret store",
    ]),
    authored_question!("reliability", "devops-rel-01", "How do you keep environments reproducible?", &[
        "Define every environment as code",
        "Rebuild an environment from its definition rather than repairing it",
        "Detect and correct configuration drift",
        "Test infrastructure changes before production",
    ]),
    authored_question!("reliability", "devops-rel-02", "How do you release changes safely?", &[
        "Release small changes often",
        "Expose a change to a fraction of traffic first",
        "Automate the rollback of a failed release",
        "Decouple deployment from feature release",
    ]),
    authored_question!("performance", "devops-perf-01", "How do you keep your delivery pipeline fast?", &[
        "Measure the lead time of each pipeline stage",
        "Run independent stages in parallel",
        "Cache dependencies and build outputs",
        "Size build capacity to the team's cadence",
    ]),
    authored_question!("costOptimization", "devops-cost-01", "How do you control the cost of delivery?", &[
        "Shut down environments nobody is using",
        "Right-size build and test capacity",
        "Attribute pipeline cost to the team that incurs it",
        "Remove pipelines and artifacts nothing consumes",
    ]),
];

/// The Games Industry Lens's question set: the lens's published focus areas, as this
/// emulator's content (see this module's docs), in pillar order.
const GAMES_LENS_QUESTIONS: &[FrameworkQuestion] = &[
    authored_question!("operationalExcellence", "games-ops-01", "How do you operate a live game service?", &[
        "Monitor the player experience rather than only the servers",
        "Run a live-operations calendar of content and events",
        "Rehearse the response to a launch-day incident",
        "Communicate service status to players",
    ]),
    authored_question!("security", "games-sec-01", "How do you protect player accounts and their data?", &[
        "Authenticate players through an identity provider",
        "Protect player personal data and age-gated information",
        "Detect and act on account takeover",
        "Audit administrative access to player data",
    ]),
    authored_question!("security", "games-sec-02", "How do you protect your game from cheating and abuse?", &[
        "Treat the client as untrusted and validate on the server",
        "Detect anomalous play and automate the response",
        "Rate-limit and authenticate the game's APIs",
        "Protect game content and build artifacts from leaking",
    ]),
    authored_question!("reliability", "games-rel-01", "How do you run game servers and matchmaking reliably?", &[
        "Place sessions close to the players in them",
        "Drain a game server without dropping its sessions",
        "Keep matchmaking working as the player population changes",
        "Contain a failure to a subset of sessions",
    ]),
    authored_question!("reliability", "games-rel-02", "How do you protect player progression?", &[
        "Make progression writes idempotent",
        "Back up player data and test its recovery",
        "Reconcile entitlements and purchases against an authoritative record",
        "Keep a path back from a bad content release",
    ]),
    authored_question!("performance", "games-perf-01", "How do you meet the latency a game needs?", &[
        "Measure latency at the percentiles players notice",
        "Choose Regions and edge locations from the player map",
        "Size capacity for a launch or an event",
        "Tune the network protocol to the game's tick rate",
    ]),
    authored_question!("costOptimization", "games-cost-01", "How do you control the cost of a game at scale?", &[
        "Scale session capacity with the player population",
        "Use interruptible capacity for tolerant workloads",
        "Attribute cost to the title and the feature",
        "Retire the infrastructure of a sunset title",
    ]),
    authored_question!("sustainability", "games-sus-01", "How do you reduce the footprint of your game workloads?", &[
        "Raise the utilization of session capacity",
        "Ship smaller updates to players",
        "Expire telemetry and replays nobody uses",
    ]),
];

