// SPDX-License-Identifier: CC0-1.0

//! The ONE profile template this emulator publishes, and the lookup every operation that
//! reports a profile question goes through.
//!
//! "Get profile template" (`smithy_query(query="operation",
//! operation="GetProfileTemplate")`) answers a single `ProfileTemplate` and takes no input
//! at all (`GetProfileTemplateInput` declares no member), so the template is SERVICE-level
//! published content: there is one of it per service, every caller reads the same one, and
//! nothing a caller does revises it. It is the catalog a client reads to discover the
//! `QuestionId`s and `ChoiceId`s that `CreateProfile`/`UpdateProfile`'s
//! `ProfileQuestionUpdate$QuestionId` and `$SelectedChoiceIds` name, and the one place the
//! text of a profile question is decided: a stored profile records WHICH question it
//! answers and the choices selected for it, while what that question SAYS — its title, its
//! description, the choices it offers and how many of them may be selected — is read off
//! here ([`profile_template_question`], which
//! [`super::ProfileRecord::profile_questions`] reports a profile's answers through). One
//! table, so the template a client reads and the question text a profile reports cannot
//! disagree.
//!
//! ## Where the content comes from
//!
//! A profile "records your business priorities" so that a review can be prioritized
//! against them, which is what the modelled `Profile`/`ProfileTemplate` pair expresses;
//! the QUESTIONS that template asks are template CONTENT, which neither the Smithy model
//! nor the service docs publish in a machine-readable form, so the ids, titles,
//! descriptions and choices below are this emulator's own, exactly as the lens question
//! sets in [`super::official_lenses`] are. Every value sits inside the bounds the modelled
//! shapes publish — `ProfileTemplate$TemplateName` is a `ProfileName`
//! (`@length(3, 100)`), `ProfileTemplateQuestion$QuestionId` `@length(1, 128)`,
//! `$QuestionTitle` `@length(1, 512)`, `$QuestionDescription` `@length(1, 1024)`,
//! `$MinSelectedChoices`/`$MaxSelectedChoices` `@range(min: 0)`, and
//! `ProfileTemplateChoice$ChoiceId` `@length(1, 64)`, `$ChoiceTitle` `@length(1, 512)`,
//! `$ChoiceDescription` `@length(1, 1024)` (`smithy_query(query="shape",
//! shape="ProfileTemplate,ProfileTemplateQuestion,ProfileTemplateChoice")`) — and a test
//! pins that, pins that each question's selection bounds are reachable within the choices
//! it offers, and pins that a discovered id round-trips through a created profile.
//!
//! The table is extensible by construction: another question the template asks is another
//! row, and nothing below it is keyed to how many rows there are.

use std::time::SystemTime;

use crate::ops::types::{
    ProfileChoice, ProfileTemplate, ProfileTemplateChoice, ProfileTemplateQuestion,
};

use super::official_lenses::midnight_utc;

/// `ProfileTemplate$TemplateName`: the name the service publishes its one profile
/// template under, within the modelled `ProfileName` `@length(3, 100)`.
pub const PROFILE_TEMPLATE_NAME: &str = "Default Profile Template";

/// The revision of the template content below, as a calendar day.
///
/// `ProfileTemplate$CreatedAt`/`$UpdatedAt` are when the template was created and last
/// updated, and for content the SERVICE publishes that is when the revision a caller reads
/// was published ([`profile_template_published_at`]) — the same reading as
/// [`super::official_lenses::AUTHORED_CONTENT_REVISION`] for a published lens. A change to
/// the questions below changes it. Read from here rather than from the clock: a template
/// nobody can revise must report the same instants to every caller.
pub const PROFILE_TEMPLATE_REVISION: &str = "2025-01-15";

/// One choice a template question offers: the shape of `ProfileTemplateChoice`, as the
/// table holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileTemplateChoiceEntry {
    /// `ProfileTemplateChoice$ChoiceId` — what a request's `SelectedChoiceIds` names the
    /// choice by, unique within the question that offers it.
    pub choice_id: &'static str,
    /// `$ChoiceTitle`: the choice, in one line.
    pub title: &'static str,
    /// `$ChoiceDescription`: what selecting it means. Optional on the shape, and every
    /// entry here carries one.
    pub description: &'static str,
}

/// One question the profile template asks: the shape of `ProfileTemplateQuestion`, as the
/// table holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProfileTemplateQuestionEntry {
    /// `ProfileTemplateQuestion$QuestionId` — the key of the table, and what a profile's
    /// answer is recorded under.
    pub question_id: &'static str,
    /// `$QuestionTitle`: the question, as it is asked.
    pub title: &'static str,
    /// `$QuestionDescription`: what the answer is used for.
    pub description: &'static str,
    /// `$QuestionChoices`: the choices this question offers, in the template's own order.
    pub choices: &'static [ProfileTemplateChoiceEntry],
    /// `$MinSelectedChoices`: the fewest choices an answer to this question selects — `0`
    /// for a question a profile may leave unanswered.
    pub min_selected_choices: i32,
    /// `$MaxSelectedChoices`: the most choices an answer to this question selects — `1`
    /// for a question that asks for exactly one of its choices.
    pub max_selected_choices: i32,
}

impl ProfileTemplateQuestionEntry {
    /// This row as the `ProfileTemplateQuestion` `GetProfileTemplate` reports.
    pub fn to_template_question(&self) -> ProfileTemplateQuestion {
        ProfileTemplateQuestion {
            question_id: Some(self.question_id.to_string()),
            question_title: Some(self.title.to_string()),
            question_description: Some(self.description.to_string()),
            question_choices: Some(
                self.choices
                    .iter()
                    .map(|choice| ProfileTemplateChoice {
                        choice_id: Some(choice.choice_id.to_string()),
                        choice_title: Some(choice.title.to_string()),
                        choice_description: Some(choice.description.to_string()),
                    })
                    .collect(),
            ),
            min_selected_choices: Some(self.min_selected_choices),
            max_selected_choices: Some(self.max_selected_choices),
        }
    }

    /// This question's choices as a PROFILE reports them (`ProfileQuestion$QuestionChoices`
    /// is a `ProfileChoice` list, carrying the same three members
    /// `ProfileTemplateChoice` does), so the catalog a profile reports and the one the
    /// template publishes are the same rows.
    pub fn choice_catalog(&self) -> Vec<ProfileChoice> {
        self.choices
            .iter()
            .map(|choice| ProfileChoice {
                choice_id: Some(choice.choice_id.to_string()),
                choice_title: Some(choice.title.to_string()),
                choice_description: Some(choice.description.to_string()),
            })
            .collect()
    }

    /// Whether `choice_id` names one of this question's choices — a choice belongs to the
    /// question that offers it, so this is what a selection is judged against.
    pub fn offers_choice(&self, choice_id: &str) -> bool {
        self.choices
            .iter()
            .any(|choice| choice.choice_id == choice_id)
    }
}

/// The questions the published profile template asks, in the template's own order: the
/// business priorities a review is weighed against first, then what the organization and
/// its workloads are, then the obligations those workloads are subject to.
///
/// The ids and the text are this emulator's own (see this module's docs); every value sits
/// inside the bounds the modelled shapes publish, which a test pins.
pub const PROFILE_TEMPLATE_QUESTIONS: &[ProfileTemplateQuestionEntry] = &[
    ProfileTemplateQuestionEntry {
        question_id: "profile-business-priorities",
        title: "Which of the following are the most important business priorities for the \
                workloads this profile is used to review?",
        description: "The priorities selected here are what a review weighs most heavily, so \
                      answer them for the business rather than for one workload.",
        choices: &[
            ProfileTemplateChoiceEntry {
                choice_id: "cost-reduction",
                title: "Reduce cost",
                description: "Spend less on the workload without giving up the outcomes it \
                              delivers.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "operational-resilience",
                title: "Improve operational resilience",
                description: "Keep the workload available, and recoverable, when a component \
                              it depends on fails.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "security-and-compliance",
                title: "Strengthen security and compliance",
                description: "Protect the data the workload holds and meet the obligations it \
                              is subject to.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "speed-to-market",
                title: "Increase speed to market",
                description: "Release changes to the workload more often, and with less risk \
                              in each release.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "performance-at-scale",
                title: "Sustain performance at scale",
                description: "Keep the workload's performance predictable as the demand on it \
                              grows.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "environmental-impact",
                title: "Reduce environmental impact",
                description: "Lower the energy and the resources the workload consumes for the \
                              work it does.",
            },
        ],
        // A review is prioritized against the priorities a profile names, so a profile that
        // names none prioritizes nothing; three is as many as can be weighed first.
        min_selected_choices: 1,
        max_selected_choices: 3,
    },
    ProfileTemplateQuestionEntry {
        question_id: "profile-industry-type",
        title: "Which industry type best describes your organization?",
        description: "An organization's industry type is what its review is compared against, \
                      so exactly one of these describes it.",
        choices: &[
            ProfileTemplateChoiceEntry {
                choice_id: "enterprise",
                title: "Enterprise",
                description: "A large organization running many workloads across several \
                              business units.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "public-sector",
                title: "Public sector",
                description: "A government, education or non-profit organization.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "small-or-medium-business",
                title: "Small or medium business",
                description: "An organization running a small number of workloads with a small \
                              operations team.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "startup",
                title: "Startup",
                description: "An organization whose workloads are still finding their market.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "independent-software-vendor",
                title: "Independent software vendor",
                description: "An organization whose workloads are the software it sells to \
                              others.",
            },
        ],
        min_selected_choices: 1,
        max_selected_choices: 1,
    },
    ProfileTemplateQuestionEntry {
        question_id: "profile-industry",
        title: "Which industry best describes the workloads this profile is used to review?",
        description: "The industry a workload serves decides which best practices a review \
                      raises first, so exactly one of these describes it.",
        choices: &[
            ProfileTemplateChoiceEntry {
                choice_id: "financial-services",
                title: "Financial services",
                description: "Banking, payments, capital markets or insurance workloads.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "healthcare-and-life-sciences",
                title: "Healthcare and life sciences",
                description: "Care delivery, medical devices, pharmaceutical or research \
                              workloads.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "retail-and-consumer-goods",
                title: "Retail and consumer goods",
                description: "Storefront, fulfilment, supply-chain or consumer-brand workloads.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "manufacturing",
                title: "Manufacturing",
                description: "Industrial, automotive or process-manufacturing workloads.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "media-and-entertainment",
                title: "Media and entertainment",
                description: "Publishing, broadcast, streaming or gaming workloads.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "education",
                title: "Education",
                description: "Teaching, research or student-administration workloads.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "government",
                title: "Government",
                description: "Citizen-service, defence or public-administration workloads.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "telecommunications",
                title: "Telecommunications",
                description: "Network, subscriber or connectivity-service workloads.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "other",
                title: "Another industry",
                description: "An industry none of the choices above describes.",
            },
        ],
        min_selected_choices: 1,
        max_selected_choices: 1,
    },
    ProfileTemplateQuestionEntry {
        question_id: "profile-workload-lifecycle",
        title: "Where in their lifecycle are the workloads this profile is used to review?",
        description: "How far a workload has got decides which best practices are worth acting \
                      on now, so exactly one of these describes it.",
        choices: &[
            ProfileTemplateChoiceEntry {
                choice_id: "planning",
                title: "Being planned",
                description: "The workload is being designed and nothing is running yet.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "in-development",
                title: "In development",
                description: "The workload is being built and is not serving traffic yet.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "pre-production",
                title: "In pre-production",
                description: "The workload is running and being validated before it serves \
                              customers.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "production",
                title: "In production",
                description: "The workload is serving customers.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "sunsetting",
                title: "Being retired",
                description: "The workload is being wound down and its traffic moved elsewhere.",
            },
        ],
        min_selected_choices: 1,
        max_selected_choices: 1,
    },
    ProfileTemplateQuestionEntry {
        question_id: "profile-regulatory-requirements",
        title: "Which regulatory or compliance requirements apply to the workloads this profile \
                is used to review?",
        description: "Each requirement selected here is one a review has to account for; a \
                      workload subject to none selects nothing.",
        choices: &[
            ProfileTemplateChoiceEntry {
                choice_id: "pci-dss",
                title: "Payment Card Industry Data Security Standard",
                description: "The workload stores, processes or transmits payment card data.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "hipaa",
                title: "Health Insurance Portability and Accountability Act",
                description: "The workload handles protected health information.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "gdpr",
                title: "General Data Protection Regulation",
                description: "The workload processes the personal data of people in the \
                              European Union.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "fedramp",
                title: "Federal Risk and Authorization Management Program",
                description: "The workload is operated for a United States federal agency.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "soc-2",
                title: "System and Organization Controls 2",
                description: "The workload is reported on against the trust-services criteria.",
            },
            ProfileTemplateChoiceEntry {
                choice_id: "iso-27001",
                title: "ISO/IEC 27001",
                description: "The workload is part of a certified information-security \
                              management system.",
            },
        ],
        // A workload is subject to the requirements it is subject to: none of them, all of
        // them, or anything between.
        min_selected_choices: 0,
        max_selected_choices: 6,
    },
];

/// The ONE question the published template asks under `question_id`, when it asks it.
///
/// `QuestionId` is how a profile's answers are keyed, so this is the lookup every reading
/// of "what does this question say?" goes through — `None` for a question the template
/// does not publish, which has no published text to report.
pub fn profile_template_question(
    question_id: &str,
) -> Option<&'static ProfileTemplateQuestionEntry> {
    PROFILE_TEMPLATE_QUESTIONS
        .iter()
        .find(|question| question.question_id == question_id)
}

/// When the revision of the template this emulator publishes was published — midnight UTC
/// of the day [`PROFILE_TEMPLATE_REVISION`] names.
pub fn profile_template_published_at() -> SystemTime {
    midnight_utc(PROFILE_TEMPLATE_REVISION)
}

/// The `ProfileTemplate` `GetProfileTemplate` reports: the published name, every question
/// of [`PROFILE_TEMPLATE_QUESTIONS`] in the template's own order, and the instants the
/// revision those questions belong to was published.
///
/// The only place the response is built, so two readings of the template cannot differ,
/// and `CreatedAt` equals `UpdatedAt` because nothing has revised what the service
/// published.
pub fn profile_template() -> ProfileTemplate {
    let published_at = ::aws_smithy_types::DateTime::from(profile_template_published_at());
    ProfileTemplate {
        template_name: Some(PROFILE_TEMPLATE_NAME.to_string()),
        template_questions: Some(
            PROFILE_TEMPLATE_QUESTIONS
                .iter()
                .map(ProfileTemplateQuestionEntry::to_template_question)
                .collect(),
        ),
        created_at: Some(published_at),
        updated_at: Some(published_at),
    }
}
