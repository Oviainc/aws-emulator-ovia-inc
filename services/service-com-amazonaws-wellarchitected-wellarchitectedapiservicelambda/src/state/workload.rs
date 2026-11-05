// SPDX-License-Identifier: CC0-1.0

use super::*;

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

    /// Drop the review of one lens from this workload: the lens itself, the answers
    /// recorded against its review and whatever a caller wrote to that review.
    ///
    /// A lens review is not a resource of its own — it exists exactly while that lens is
    /// associated with the workload ([`Self::reviews_lens`]) — so dropping the lens drops
    /// the review, and the answers and notes held under it go with it rather than
    /// surviving to reappear under a later association. A milestone records the workload
    /// BY VALUE, so a milestone taken before this still reports the review as it stood.
    ///
    /// Idempotent in the lens and in the SPELLING of it: a lens is one lens however it is
    /// named ([`lens_alias`]), so every entry naming the same lens goes and the order of
    /// the rest is left alone. It answers nothing, because whether the workload reviewed
    /// the lens is what [`Self::reviews_lens`] is for — an operation that has to refuse a
    /// lens the workload does not review resolves it BEFORE removing any.
    pub fn disassociate_lens(&mut self, lens: &str) {
        let alias = lens_alias(lens);
        self.lenses.retain(|attached| lens_alias(attached) != alias);
        self.answers.remove(alias);
        self.lens_review_records.remove(alias);
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

    /// The lens reviews this workload has, through the one place that rule lives
    /// ([`lens_reviews_of`]): every attached lens's [`lens_alias`] beside the name the
    /// attach recorded it under, in the order the workload applies its lenses — the order
    /// a page over these reviews follows.
    pub fn lens_reviews(&self) -> Vec<(&str, &str)> {
        lens_reviews_of(&self.lenses)
    }

    /// Every lens this workload reviews, as the attach recorded it, beside the
    /// `LensVersion` its review of that lens is against
    /// ([`Self::lens_review_version`]) — what resolving the content of this workload's
    /// reviews takes
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_content`]),
    /// because a lens review is a review against ONE version of the lens.
    pub fn reviewed_lens_versions(&self) -> Vec<(&str, Option<&str>)> {
        self.lenses
            .iter()
            .map(|lens| (lens.as_str(), self.lens_review_version(lens)))
            .collect()
    }

    /// This workload's risk roll-up: one entry for every `Risk` the enum publishes, so
    /// a caller that reads a bucket to report review progress always finds it.
    ///
    /// `UNANSWERED` counts the questions of every lens attached to the workload that
    /// carries no answer yet; each other bucket counts the stored answers that carry
    /// that risk ([`AnswerRecord::risk`]). The map is DERIVED from the attached lenses
    /// and the stored answers on every read and never stored, so it moves when either
    /// does.
    ///
    /// `content` is the content of the lenses this workload reviews, resolved from the
    /// whole lens catalog
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_content`]):
    /// what a lens asks is that lens's own content, whichever half of the catalog holds
    /// it.
    pub fn risk_counts(&self, content: &ReviewedLensContent) -> BTreeMap<String, i32> {
        let mut counts = empty_risk_counts();
        // A lens is attached once however many ways the request named it, so count the
        // questions of each lens the workload reviews once ([`Self::lens_reviews`] is the
        // one place that identity rule lives).
        for (lens, _) in self.lens_reviews() {
            for (risk, count) in self.lens_risk_counts(lens, content) {
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
    /// `lens` is a lens as a request names it (an alias or an ARN), and `content` is as
    /// [`Self::risk_counts`] takes it. A lens this emulator holds no content for asks
    /// nothing, so every bucket is 0.
    pub fn lens_risk_counts(
        &self,
        lens: &str,
        content: &ReviewedLensContent,
    ) -> BTreeMap<String, i32> {
        self.risk_counts_over(lens, content.of(lens).questions().iter())
    }

    /// The risk roll-up of ONE pillar of one lens review: [`Self::lens_risk_counts`]
    /// narrowed to the questions that pillar asks, which is what a pillar review summary
    /// of a lens review reports.
    pub fn pillar_risk_counts(
        &self,
        lens: &str,
        pillar_id: &str,
        content: &ReviewedLensContent,
    ) -> BTreeMap<String, i32> {
        self.risk_counts_over(lens, content.of(lens).pillar_questions(pillar_id))
    }

    /// The risk roll-up over an arbitrary set of ONE lens's questions — the one place a
    /// risk is counted, so the workload's roll-up, a lens review's and a pillar's cannot
    /// disagree about what a question counts as.
    fn risk_counts_over<'question>(
        &self,
        lens: &str,
        questions: impl Iterator<Item = &'question ReviewQuestion>,
    ) -> BTreeMap<String, i32> {
        let mut counts = empty_risk_counts();
        for question in questions {
            let risk = match self.answer(lens, &question.question_id) {
                Some(answer) => answer.risk(question),
                None => RISK_UNANSWERED,
            };
            counts
                .entry(risk.to_string())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
        }
        counts
    }

    /// ONE of this workload's lens reviews as the store holds it, if the store holds one.
    ///
    /// `lens` is a lens as a request names it — an alias or an ARN — and resolves through
    /// [`lens_alias`], so the same record is found by either name.
    pub fn lens_review_record(&self, lens: &str) -> Option<&LensReviewRecord> {
        self.lens_review_records.get(lens_alias(lens))
    }

    /// [`Self::lens_review_record`] for a caller about to WRITE it, created empty on the
    /// first write: the lens review itself exists as soon as the lens is associated with
    /// the workload, and this record is only how the store remembers that review's own
    /// state.
    pub fn lens_review_record_mut(&mut self, lens: &str) -> &mut LensReviewRecord {
        self.lens_review_records
            .entry(lens_alias(lens).to_string())
            .or_default()
    }

    /// Whether this workload leaves the choice of which questions carry an issue in the
    /// integrated service to the CALLER.
    ///
    /// `IssueManagementType` publishes `AUTO` and `MANUAL`
    /// (`smithy_query(query="shape", shape="IssueManagementType")`): under
    /// [`ISSUE_MANAGEMENT_TYPE_MANUAL`] the questions covered are the ones the review's own
    /// selection names, and under `AUTO` \u2014 and for a workload that records no type, which
    /// has selected nothing to manage by hand \u2014 every question of the review is covered.
    pub fn jira_issue_management_is_manual(&self) -> bool {
        self.jira_configuration
            .as_ref()
            .and_then(|configuration| configuration.issue_management_type.as_deref())
            == Some(ISSUE_MANAGEMENT_TYPE_MANUAL)
    }

    /// The `QuestionId`s of ONE of this workload's lens reviews the caller has selected for
    /// the integrated service (`LensReview$JiraConfiguration` ->
    /// `SelectedPillar$SelectedQuestionIds`), EMPTY when the review selects none.
    ///
    /// `lens` is a lens as a request names it, so the selection is found by either
    /// spelling ([`lens_review_record`](Self::lens_review_record)). The pillar a question
    /// is selected under is not re-checked here: the selection is recorded against the
    /// review by `UpdateLensReview`, which refuses a pillar or question the lens does not
    /// define, so what is stored already belongs to this lens.
    pub fn jira_selected_question_ids(&self, lens: &str) -> BTreeSet<String> {
        self.lens_review_record(lens)
            .and_then(|written| written.jira_configuration.as_ref())
            .and_then(|configuration| configuration.selected_pillars.as_ref())
            .map(|pillars| {
                pillars
                    .iter()
                    .filter_map(|pillar| pillar.selected_question_ids.as_ref())
                    .flatten()
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The `JiraConfiguration` one question of one of this workload's lens reviews
    /// reports: the integration state of that question's issue, absent until a sync has
    /// recorded one ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::sync_workload_integration`]).
    ///
    /// `LastSyncedTime` is "the date and time when the Jira configuration was last synced"
    /// (`smithy_query(query="shape", shape="JiraConfiguration")`), so it is the instant the
    /// sync that covered this question was served at. `JiraIssueUrl` is omitted: the issue
    /// it addresses lives in the account's own Jira site, which this emulator holds nothing
    /// of ([`GlobalSettingsRecord::to_global_settings`]) \u2014 the member is optional, so it is
    /// left off rather than filled with a URL that addresses nothing.
    ///
    /// Lives on the record because the single answer read, the answer list and the
    /// improvement plan all report it, so the three cannot disagree about when a question
    /// was synced.
    pub fn question_jira_configuration(
        &self,
        lens: &str,
        question_id: &str,
    ) -> Option<JiraConfiguration> {
        let synced_at = *self
            .lens_review_record(lens)?
            .jira_question_syncs
            .get(question_id)?;
        Some(JiraConfiguration {
            jira_issue_url: None,
            last_synced_time: Some(::aws_smithy_types::DateTime::from(synced_at)),
        })
    }

    /// The `LensVersion` ONE lens review of this workload is against, as it was pinned
    /// when the lens was associated or last moved by an upgrade — the mirror of
    /// [`ReviewTemplateRecord::lens_review_version`], because a workload's lens review and
    /// a review template's are both reviews against ONE version of the lens.
    pub fn lens_review_version(&self, lens: &str) -> Option<&str> {
        self.lens_review_record(lens)?.lens_version.as_deref()
    }

    /// The pillars ONE lens review of this workload reports a summary for, through the
    /// one place that rule lives ([`lens_review_pillars`]).
    pub fn lens_review_pillars(
        &self,
        lens: &str,
        content: &ReviewedLensContent,
    ) -> Vec<(String, Option<String>)> {
        lens_review_pillars(
            content.of(lens),
            self.lens_review_record(lens)
                .map(|written| &written.pillar_notes),
        )
    }

    /// The `LensReview` shape `GetLensReview` and `UpdateLensReview` return for ONE lens
    /// associated with this workload.
    ///
    /// `lens` is the lens AS THE REQUEST NAMED IT — `LensAlias` is "either the lens alias
    /// … or the lens ARN" (`LensAlias` doc) and this shape models no member to resolve one
    /// spelling into the other — and `reviewed` is what the lens catalog says about the
    /// lens this review is against, resolved for the version the review is at
    /// ([`ComAmazonawsWellarchitectedWellarchitectedapiservicelambdaState::reviewed_lens_at`]).
    ///
    /// Everything but the review's own state ([`LensReviewRecord`]) is DERIVED on every
    /// read: the lens's name, version and currency from `reviewed`, one pillar summary per
    /// pillar of the lens ([`Self::lens_review_pillars`]) and the risk roll-ups of the
    /// lens's and each pillar's questions from this workload's stored answers — so a
    /// review moves when an answer does.
    pub fn to_lens_review(
        &self,
        lens: &str,
        reviewed: &ReviewedLens,
        content: &ReviewedLensContent,
    ) -> LensReview {
        let written = self.lens_review_record(lens);
        LensReview {
            lens_alias: Some(lens.to_string()),
            lens_arn: Some(reviewed.lens_arn.clone()),
            lens_version: reviewed.lens_version.clone(),
            lens_name: reviewed.lens_name.clone(),
            lens_status: reviewed.lens_status.clone(),
            pillar_review_summaries: Some(
                self.lens_review_pillars(lens, content)
                    .into_iter()
                    .map(|(pillar_id, pillar_name)| PillarReviewSummary {
                        notes: written
                            .and_then(|written| written.pillar_notes.get(&pillar_id).cloned()),
                        risk_counts: Some(risk_counts_to_map(
                            &self.pillar_risk_counts(lens, &pillar_id, content),
                        )),
                        // The risk counts restricted to the questions a PROFILE
                        // prioritises are not projected for a pillar: which questions
                        // those are needs a mapping of the published profile template's
                        // choices onto a lens's questions that nothing publishes (see
                        // [`DEFAULT_QUESTION_TYPE`]), so no question of any workload is
                        // prioritized.
                        prioritized_risk_counts: None,
                        pillar_id: Some(pillar_id),
                        pillar_name,
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
            risk_counts: Some(risk_counts_to_map(&self.lens_risk_counts(lens, content))),
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

    /// Whether this workload's Trusted Advisor integration is activated: whether the
    /// discovery configuration it reports carries
    /// [`TRUSTED_ADVISOR_INTEGRATION_ENABLED`].
    ///
    /// It is what the Trusted Advisor checks of this workload's best practices are fetched
    /// under — "discovery integration status in respect to Trusted Advisor for the
    /// workload" (`smithy_query(query="shape", shape="WorkloadDiscoveryConfig")`) — so a
    /// workload nobody activated it for has no check to report, and activating it (through
    /// `CreateWorkload`'s or `UpdateWorkload`'s `DiscoveryConfig`) is what makes the checks
    /// of a best practice appear.
    pub fn trusted_advisor_integration_enabled(&self) -> bool {
        self.effective_discovery_config()
            .trusted_advisor_integration_status
            .as_deref()
            == Some(TRUSTED_ADVISOR_INTEGRATION_ENABLED)
    }

    /// The accounts a Trusted Advisor check of this workload is reported for, in reporting
    /// order: the account that OWNS the workload, then each account the workload's
    /// `AccountIds` names, in the workload's own order.
    ///
    /// A check detail is reported "by account related to the workload"
    /// (`smithy_query(query="operation", operation="ListCheckDetails")`), and the accounts
    /// related to a workload are the one it lives in and the ones it was created with
    /// ("the list of Amazon Web Services account IDs associated with the workload" —
    /// `smithy_query(query="shape", shape="Workload", member="AccountIds")`). Distinct in
    /// the account id, so a workload that lists its own account, or lists one twice, is
    /// reported for it once.
    pub fn check_account_ids(&self) -> Vec<String> {
        // Membership is tested against a set rather than by scanning what is already kept:
        // `AccountIds` is a list the caller chose the length of, so a scan per entry would
        // make building the reporting order cost work quadratic in that length.
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut accounts: Vec<String> = Vec::with_capacity(self.account_ids.len() + 1);
        for account_id in std::iter::once(&self.owner).chain(self.account_ids.iter()) {
            if seen.insert(account_id.as_str()) {
                accounts.push(account_id.clone());
            }
        }
        accounts
    }

    /// The `Workload` shape `GetWorkload`/`UpdateWorkload` return. `content` is the
    /// content of the lenses this workload reviews, which its risk roll-up is counted over
    /// ([`Self::risk_counts`]).
    pub fn to_workload(&self, content: &ReviewedLensContent) -> Workload {
        Workload {
            // Whether this workload is supported for a Foundational Technical Review. No
            // operation this service exposes enrols a workload in that review, and no
            // lens in this emulator's catalog is its lens, so no workload this store
            // holds is FTR-supported. It is REPORTED rather than omitted, so a caller
            // reading the flag to decide whether to offer the workflow can tell "not
            // supported" from "unknown".
            ftr_supported: Some(false),
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
            risk_counts: Some(risk_counts_to_map(&self.risk_counts(content))),
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

    /// The `WorkloadSummary` projection `ListWorkloads` pages over. `content` is as
    /// [`Self::to_workload`] takes it.
    pub fn to_summary(&self, content: &ReviewedLensContent) -> WorkloadSummary {
        WorkloadSummary {
            workload_id: Some(self.workload_id.clone()),
            workload_arn: Some(self.workload_arn.clone()),
            workload_name: Some(self.workload_name.clone()),
            owner: Some(self.owner.clone()),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            lenses: Some(self.lenses.clone()),
            risk_counts: Some(risk_counts_to_map(&self.risk_counts(content))),
            improvement_status: Some(self.improvement_status.clone()),
            // As in [`Self::to_workload`]: the associations are projected from the
            // record, the prioritised roll-up has nothing to compute it from.
            profiles: self.profiles_projection(),
            prioritized_risk_counts: None,
        }
    }
}
