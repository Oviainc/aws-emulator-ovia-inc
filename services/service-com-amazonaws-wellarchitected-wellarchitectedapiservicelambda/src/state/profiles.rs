// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The question answers a request's `ProfileQuestionUpdates` record, keyed by the question
/// each update answers — or the modelled `ValidationException` (HTTP 400) for an entry no
/// profile can record.
///
/// The updates are answers to questions, so each one is recorded UNDER the question it
/// answers (`ProfileQuestionUpdate$QuestionId`): the request's list becomes the map a
/// profile holds. Two entries the model allows cannot be recorded that way, and both are
/// refused rather than silently dropped or merged: an update that names no question
/// (`QuestionId` is optional) answers nothing, and two updates naming the SAME question
/// disagree about one answer. The selected choices, by contrast, are a SET — "the selected
/// choices" — so a choice is either selected or not and naming one twice in one update
/// selects it once.
///
/// Both identifiers an update carries come from the ONE profile template this service
/// publishes ([`PROFILE_TEMPLATE_QUESTIONS`], which `GetProfileTemplate` serves and which
/// is the only place a client discovers either), so each is RESOLVED against it: a
/// `QuestionId` naming a question the template does not ask answers no question of this
/// service, and a `SelectedChoiceIds` entry naming no choice of the question it answers
/// selects nothing of that question. Either is refused rather than stored — the rule every
/// other identifier taken from a published catalog follows here
/// ([`undefined_choice_refusal`], [`undefined_pillar_refusal`]) — so a stored profile's
/// answers and the question text reported beside them
/// ([`ProfileRecord::profile_questions`]) cannot disagree.
///
/// Shared by the two operations that record a profile's answers from this member
/// (`CreateProfile` and `UpdateProfile`), so neither can normalise or refuse one
/// differently; the refusal is worded the way the framework's own constraint walk words a
/// member failure ([`member_constraint_exception`]), so a handler-enforced rule reads like
/// a modelled one. The offending choice ids are sorted and deduplicated, so a refusal does
/// not depend on the order the request listed them in.
pub fn recorded_profile_questions(
    updates: &[crate::ops::types::ProfileQuestionUpdate],
) -> Result<BTreeMap<String, BTreeSet<String>>, crate::ops::types::ValidationException> {
    let mut questions: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for update in updates {
        let question_id = update.question_id.as_deref().unwrap_or_default();
        if question_id.is_empty() {
            return Err(member_constraint_exception(
                PROFILE_QUESTIONS_MEMBER,
                "Member must name the question it answers in QuestionId",
            ));
        }
        if questions.contains_key(question_id) {
            return Err(member_constraint_exception(
                PROFILE_QUESTIONS_MEMBER,
                &format!(
                    "Member must answer each question at most once, got {question_id} twice"
                ),
            ));
        }
        let published = profile_template_question(question_id).ok_or_else(|| {
            member_constraint_exception(
                PROFILE_QUESTIONS_MEMBER,
                &format!(
                    "every question must be a question the profile template asks, which \
                     {question_id} is not"
                ),
            )
        })?;
        let selected = update.selected_choice_ids.clone().unwrap_or_default();
        let mut undefined: Vec<&str> = selected
            .iter()
            .map(String::as_str)
            .filter(|choice_id| !published.offers_choice(choice_id))
            .collect();
        if !undefined.is_empty() {
            undefined.sort_unstable();
            undefined.dedup();
            return Err(member_constraint_exception(
                PROFILE_QUESTIONS_MEMBER,
                &format!(
                    "every choice must be a choice of question {question_id}, which {} is not",
                    undefined.join(", ")
                ),
            ));
        }
        questions.insert(question_id.to_string(), selected.into_iter().collect());
    }
    Ok(questions)
}

/// `ProfileOwnerType$SELF`: a profile the CALLING account owns
/// (`smithy_query(query="shape", shape="ProfileOwnerType")`, which publishes `SELF` and
/// `SHARED`).
pub const PROFILE_OWNER_TYPE_SELF: &str = "SELF";

/// `ProfileOwnerType$SHARED`: a profile another account owns and shared with the caller.
pub const PROFILE_OWNER_TYPE_SHARED: &str = "SHARED";

/// `ProfileNotificationType$PROFILE_ANSWERS_UPDATED`: the profile a workload is reviewed
/// against has been revised since the workload's association pinned a version of it
/// (`smithy_query(query="shape", shape="ProfileNotificationType")`, which publishes
/// `PROFILE_ANSWERS_UPDATED` and `PROFILE_DELETED`).
///
/// The other member cannot arise from the state this emulator keeps: an association names
/// a profile that has to resolve, so [`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::remove_profile`]
/// drops every association of a deleted profile with the profile itself, leaving no
/// association that points at one that is gone.
pub const PROFILE_NOTIFICATION_ANSWERS_UPDATED: &str = "PROFILE_ANSWERS_UPDATED";

/// `NotificationType$LENS_VERSION_UPGRADED`: the lens a resource's lens review is a review
/// of has published a version later than the one that review is pinned at
/// (`smithy_query(query="shape", shape="NotificationType")`, which publishes
/// `LENS_VERSION_UPGRADED` and `LENS_VERSION_DEPRECATED`).
///
/// The other member cannot arise from the state this emulator keeps: no lens in either half
/// of the catalog is ever retired — an AWS-official lens is one of the published aliases
/// ([`AWS_OFFICIAL_LENS_ALIASES`]) for the life of the process, and a custom lens's versions
/// only ever accumulate — so no lens review is a review of a deprecated lens.
pub const LENS_NOTIFICATION_VERSION_UPGRADED: &str = "LENS_VERSION_UPGRADED";

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
            // "The ID assigned to the share invitation": only a profile reached THROUGH a
            // share invitation carries one. An invitation is offered to the principal a
            // profile was shared with, and this emulator attributes every request to the
            // one account that owns the profiles it holds, so a profile read here is never
            // reached through one.
            share_invitation_id: None,
            // The tag map the profile carries, empty when it carries none: the
            // `@length(min: 1)` on `TagMap` constrains the tags a REQUEST supplies, not
            // what a projection of the stored profile reports.
            tags: Some(self.tags.clone().into_iter().collect()),
        }
    }

    /// The `ProfileSummary` projection the profile LIST reports
    /// (`ListProfilesOutput$ProfileSummaries`), so one place decides how a stored profile
    /// is summarised.
    ///
    /// `ProfileSummary` is its own structure (`smithy_query(query="shape",
    /// shape="ProfileSummary")`) carrying the members it declares and no other: the
    /// questions, the tags and the share invitation `Profile` also carries are NOT among
    /// them. Every member it does declare is read off the same record [`Self::to_profile`]
    /// reads, so a listed profile cannot disagree with a read of it.
    pub fn to_summary(&self) -> ProfileSummary {
        ProfileSummary {
            profile_arn: Some(self.profile_arn.clone()),
            profile_version: Some(self.profile_version.clone()),
            profile_name: Some(self.profile_name.clone()),
            profile_description: Some(self.profile_description.clone()),
            owner: Some(self.owner.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }

    /// Whether this profile is owned by `account`, which is what a `ProfileOwnerType` of
    /// [`PROFILE_OWNER_TYPE_SELF`] selects — the complement,
    /// [`PROFILE_OWNER_TYPE_SHARED`], being a profile another account owns and shared
    /// with the caller. `Profile$Owner` is the account that created it, so ownership is
    /// decided by that member rather than by where the record is held.
    pub fn owned_by(&self, account: &str) -> bool {
        self.owner == account
    }

    /// The stored questions as the modelled `Profile$ProfileQuestions` carries them.
    ///
    /// `QuestionTitle`, `QuestionDescription`, `QuestionChoices`, `MinSelectedChoices`
    /// and `MaxSelectedChoices` describe the question itself, and the service reads them
    /// off the profile TEMPLATE it publishes (the same members on
    /// `ProfileTemplateQuestion` — `smithy_query(query="shape",
    /// shape="ProfileTemplateQuestion")`), so they are read off this emulator's published
    /// template ([`profile_template_question`]) rather than restated here: what the
    /// profile itself records is the question it answers and the choices selected for it.
    /// Every question a profile records is one that template asks — both writers resolve
    /// the identifier against it ([`recorded_profile_questions`]) — so the published row is
    /// there to read; the members stay optional because the modelled shape declares them
    /// so, and nothing is invented for a row that is not there.
    pub fn profile_questions(&self) -> Vec<ProfileQuestion> {
        self.questions
            .iter()
            .map(|(question_id, selected)| {
                let published = profile_template_question(question_id);
                ProfileQuestion {
                    question_id: Some(question_id.clone()),
                    selected_choice_ids: Some(selected.iter().cloned().collect()),
                    question_title: published.map(|question| question.title.to_string()),
                    question_description: published
                        .map(|question| question.description.to_string()),
                    question_choices: published
                        .map(ProfileTemplateQuestionEntry::choice_catalog),
                    min_selected_choices: published.map(|question| question.min_selected_choices),
                    max_selected_choices: published.map(|question| question.max_selected_choices),
                }
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
