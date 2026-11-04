// SPDX-License-Identifier: CC0-1.0

//! How this service words a request that NAMES lens content it cannot resolve.
//!
//! A lens a request asks to APPLY, the lens whose review of a workload or of a review
//! template a request addresses, and the question or the choice of such a review are
//! VALUES the request carries, not resources this API identifies. The resources it
//! identifies are the workload, the review template, the lens itself, the profile and the
//! milestone — each named by a member that resolves to a stored record on its own. So a
//! value of the first kind that resolves to nothing is INPUT the service refuses:
//! `ValidationException` (HTTP 400), carrying `Reason` and the `Fields` entry that names
//! the member the value came in, which is what tells a caller to fix the REQUEST.
//! `ResourceNotFoundException` (HTTP 404) stays what it is — the answer for a resource of
//! the second kind that the store does not hold — so a client can still tell "fix the
//! request" from "fix the state", and an SDK's modelled `ValidationException` catch fires
//! for the first.
//!
//! It is the same rule the Trusted Advisor check walk already applies to the pillar, the
//! question and the choice a check is read by
//! ([`crate::state::ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::best_practice_checks`]),
//! stated here for the lens and the lens review as well and shared by every operation
//! that resolves one, so those operations cannot word the same failure differently. Each
//! operation still wraps what it gets back in its OWN modelled error enum.
//!
//! The envelope — this service's `[Validation]` message tag, `Reason` and the `Fields`
//! list — is built by the one place this service builds a field validation
//! ([`crate::state::field_validation_exception`]), so what lives here is the SENTENCES
//! these failures are worded with and the rule that decides them.

use std::collections::HashMap;

use crate::ops::types::{ChoiceUpdate, ValidationException, ValidationExceptionField};
use crate::state;

/// The member a finding about the lens of a review names: `LensAlias`, the member every
/// operation that addresses one lens review carries it in (`GetLensReviewInput$LensAlias`,
/// `GetAnswerInput$LensAlias`, `GetReviewTemplateLensReviewInput$LensAlias`, …).
pub const LENS_ALIAS_MEMBER: &str = "LensAlias";

/// The `Message` of a refusal whose findings are ALL in `Fields`: the message states the
/// class of the failure and each finding states itself, so the sentence carries no
/// variable part.
const FIELDS_FAILED_SENTENCE: &str = "The validation of one or more fields has failed.";

/// `sentence` behind this service's failure-class tag, from the one place this service
/// tags a validation message ([`state::unclassified_exception`]), so the tag is not
/// restated here.
fn tagged(sentence: &str) -> String {
    state::unclassified_exception(sentence.to_string()).message
}

/// The refusal for a list of lenses a request asks to APPLY that the store will not
/// apply ([`state::LensesRefusal`]).
///
/// A lens is applied BY NAME, and the name is a value of the request: a name that
/// resolves to no lens is refused as that member's value rather than reported as a
/// missing resource, so a caller is told the same way whether the name is unknown or
/// names a lens no version has been published from (the refusal the store already words
/// as a field validation, passed through unchanged).
pub fn unapplicable_lenses(refusal: state::LensesRefusal) -> ValidationException {
    match refusal {
        state::LensesRefusal::NotFound(unresolved) => ValidationException {
            // The store reports the entry it could not resolve as that lookup's
            // `ResourceId`, which is the alias AS SENT — how the caller named the lens.
            message: tagged(&format!(
                "Failed to get lenses {}. User might be not authorized to access lens.",
                unresolved.resource_id
            )),
            // This refusal is not a per-member finding: the service carries `Reason`
            // without classifying it, so a client branching on the member reads "no
            // classification" rather than a field validation it would then look in
            // `Fields` for.
            reason: Some(String::new()),
            fields: None,
        },
        state::LensesRefusal::Unapplicable(exception) => exception,
    }
}

/// The refusal for a `LensAlias` that names no lens review of the workload the request
/// addresses.
///
/// A lens review of a workload exists only while that lens is associated with it
/// ([`state::WorkloadRecord::reviews_lens`]), so the alias is judged against the lenses
/// the workload carries; both the alias and the workload id are named as the request sent
/// them.
pub fn no_lens_review(workload_id: &str, lens: &str) -> ValidationException {
    state::field_validation_exception(&[(
        LENS_ALIAS_MEMBER.to_string(),
        format!("No lens review for lens alias {lens} was found in workload {workload_id}."),
    )])
}

/// The refusal for a `LensAlias` that names no lens review of the review template the
/// request addresses.
///
/// The same rule as [`no_lens_review`] on the resource that holds the other kind of lens
/// review ([`state::ReviewTemplateRecord::applies_lens`]), so the finding names the same
/// member and reads the same way, with the template as the request named it.
pub fn no_review_template_lens_review(template_arn: &str, lens: &str) -> ValidationException {
    state::field_validation_exception(&[(
        LENS_ALIAS_MEMBER.to_string(),
        format!(
            "No lens review for lens alias {lens} was found in review template {template_arn}."
        ),
    )])
}

/// The refusal for a `QuestionId` the lens under review does not ask.
///
/// The questions of a review are the LENS's own content, as of the version the review is
/// pinned at, so the finding names the lens the question was looked for in rather than
/// the workload or template whose review it is: the same `QuestionId` is unknown in that
/// lens wherever it is reviewed.
pub fn no_question(lens: &str, question_id: &str) -> ValidationException {
    state::field_validation_exception(&[(
        state::QUESTION_ID_MEMBER.to_string(),
        format!("No question with ID {question_id} was found in the lens with alias {lens}."),
    )])
}

/// The refusal for a `PillarId` a request supplies that the lens under review does not
/// define — or `None` when it names a pillar of that lens, carries none, or names a lens
/// this emulator holds no content for, which has no pillar set to judge a value against.
///
/// The pillars of a lens are that lens's own content (`content`, the lens as of the version
/// the review is pinned at), so a value naming none of them belongs to no pillar of this
/// review: it SELECTS nothing because there is nothing of that name to select, which is a
/// bad value rather than an empty answer set.
pub fn undefined_pillar(
    lens: &str,
    content: &state::LensContentView,
    pillar_id: Option<&str>,
) -> Option<ValidationException> {
    let pillar_id = pillar_id?;
    let defined = content.pillar_ids();
    if defined.is_empty() || defined.contains(&pillar_id) {
        return None;
    }
    Some(no_pillar(lens, pillar_id))
}

/// The finding [`undefined_pillar`] reports: worded like the other refusals about lens
/// content, because that is what a `PillarId` names.
fn no_pillar(lens: &str, pillar_id: &str) -> ValidationException {
    state::field_validation_exception(&[(
        state::PILLAR_ID_MEMBER.to_string(),
        format!("No pillar with ID {pillar_id} was found in the lens with alias {lens}."),
    )])
}

/// The refusal for the `ChoiceId`s an answer-recording request names that the question it
/// answers does not offer, if either member that carries one does — or `None` when every
/// id named is a choice of that question.
///
/// A choice belongs to the question that offers it, and the question's own choice catalog
/// (`Answer$Choices` / `ReviewTemplateAnswer$Choices`) is the only place a client
/// discovers those ids, so the finding hands that catalog back: the valid ids, in the
/// lens's own order. The members are judged in a fixed order, so the member a refusal
/// names does not depend on the request map's iteration order. Shared by both operations
/// that record an answer, so they cannot word it differently.
pub fn undefined_choices(
    question: &state::ReviewQuestion,
    selected_choices: Option<&[String]>,
    choice_updates: Option<&HashMap<String, ChoiceUpdate>>,
) -> Option<ValidationException> {
    // Each member's ids are BORROWED from the request and only recognised, never
    // collected, so a request whose choices are all valid allocates nothing.
    let named: [(&str, Box<dyn Iterator<Item = &str>>); 2] = [
        (
            state::SELECTED_CHOICES_MEMBER,
            Box::new(
                selected_choices
                    .unwrap_or_default()
                    .iter()
                    .map(String::as_str),
            ),
        ),
        (
            state::CHOICE_UPDATES_MEMBER,
            Box::new(
                choice_updates
                    .into_iter()
                    .flat_map(|updates| updates.keys().map(String::as_str)),
            ),
        ),
    ];
    for (member, mut choice_ids) in named {
        if choice_ids.any(|choice_id| !question.defines_choice(choice_id)) {
            return Some(invalid_choices(member, question));
        }
    }
    None
}

/// One member's finding for [`undefined_choices`]: the member that carried the bad id,
/// and the question's whole choice catalog as the valid values.
fn invalid_choices(member: &str, question: &state::ReviewQuestion) -> ValidationException {
    let valid: Vec<&str> = question
        .best_practices
        .iter()
        .map(|best_practice| best_practice.id.as_str())
        .collect();
    ValidationException {
        message: tagged(FIELDS_FAILED_SENTENCE),
        reason: Some(state::REASON_FIELD_VALIDATION_FAILED.to_string()),
        fields: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message: format!(
                "{member} contains invalid choices. Valid choices are: [{}].",
                valid.join(", ")
            ),
        }]),
    }
}
