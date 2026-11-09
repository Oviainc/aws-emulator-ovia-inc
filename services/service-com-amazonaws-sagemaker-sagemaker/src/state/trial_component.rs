// SPDX-License-Identifier: CC0-1.0

//! The experiment-tracking slice of this service's state: the [`TrialComponent`] record,
//! its bounds, and the name-or-ARN readings every trial and component operation shares.

use super::*;

/// The ARN resource kind a trial component carries, i.e. the `<kind>` in
/// [`resource_arn`]. Fixed by `TrialComponentArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment-trial-component/`), and
/// named once here so the create and the describe mint and match the same ARN.
pub const TRIAL_COMPONENT_KIND: &str = "experiment-trial-component";

/// See [`MODEL_QUOTA`]. Bounds the trial-component store the same way, and is what
/// `CreateTrialComponent` answers `ResourceLimitExceeded` past.
pub const TRIAL_COMPONENT_QUOTA: usize = 1_000;

/// See [`ASSOCIATION_QUOTA`], which bounds the lineage-edge store for the same reason and
/// at the same figure: a trial-component membership is an EDGE too — the index grows with
/// the PAIRS of trial and component a caller attaches, not with either resource — so it is
/// higher than the per-resource bounds, and it is what `AssociateTrialComponent` answers
/// `ResourceLimitExceeded` past.
pub const TRIAL_COMPONENT_TRIAL_QUOTA: usize = 10_000;

/// A trial component created by `CreateTrialComponent`, as `DescribeTrialComponent`
/// replays it.
///
/// `start_time` / `end_time` keep the modelled `Timestamp` the CALLER sent rather than a
/// `SystemTime`, so a describe replays the instant bit-for-bit however much precision the
/// request carried; `creation_time` / `last_modified_time` are the emulator's own clock
/// readings, so they are `SystemTime` like every other record here. `created_by` is stored
/// at creation rather than rebuilt per read, so every later read reports the identity that
/// actually created the component. `parameters`, `input_artifacts` and `output_artifacts`
/// are the collections the component HOLDS, empty while nothing has put an entry in them
/// (see this module's note on an entity's own collections).
#[derive(Debug, Clone)]
pub struct TrialComponent {
    pub name: String,
    pub arn: String,
    /// The Region the create signed for, kept beside the ARN the way [`Artifact`] and
    /// [`Action`] keep theirs: a trial component is a lineage entity, so a reader reports
    /// the lineage group of the Region the component was created in (see
    /// [`lineage_group_arn`]) rather than of the Region the READER happens to sign for.
    pub region: String,
    /// Never `None`: `DisplayName`'s model doc says "If DisplayName isn't specified,
    /// TrialComponentName is displayed", so a create without one stores the name.
    pub display_name: String,
    pub status: Option<crate::ops::types::TrialComponentStatus>,
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
    pub parameters: HashMap<String, crate::ops::types::TrialComponentParameterValue>,
    pub input_artifacts: HashMap<String, crate::ops::types::TrialComponentArtifact>,
    pub output_artifacts: HashMap<String, crate::ops::types::TrialComponentArtifact>,
    pub metadata_properties: Option<MetadataProperties>,
    pub created_by: UserContext,
    /// Who last modified the component: the creating identity until a call modifies it,
    /// when `UpdateTrialComponent` records the identity behind that call.
    pub last_modified_by: UserContext,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// The JOB this component tracks, as `DescribeTrialComponent`'s `Source` and
    /// `ListTrialComponents`' `SourceArn` filter report it: the job's ARN and the source
    /// type of its kind.
    ///
    /// `None` for a component `CreateTrialComponent` made, which models no source — only
    /// a job the service created a component FOR has one (see
    /// [`ComAmazonawsSagemakerSagemakerState::record_job_trial_component`]).
    pub source: Option<crate::ops::types::TrialComponentSource>,
}

/// What SageMaker records on the trial component it creates for a job submitted with an
/// `ExperimentConfig`: the `TrialComponentSource$SourceType` it reports for that kind of
/// job, and the suffix the component's generated name carries.
///
/// One value per job kind whose create models an `ExperimentConfig`
/// (`CreateTrainingJob`, `CreateProcessingJob`, `CreateTransformJob` — the three
/// `ExperimentConfig`-carrying creates this service implements), so the naming and the
/// source type of a tracked job are stated once rather than in each handler.
#[derive(Debug, Clone, Copy)]
pub struct TrackedJobKind {
    /// The `SourceType` the component reports for a job of this kind.
    pub source_type: &'static str,
    /// What the component's name carries after the job's own name.
    pub name_suffix: &'static str,
}

/// [`TrackedJobKind`] for a training job.
pub const TRAINING_JOB_TRACKING: TrackedJobKind = TrackedJobKind {
    source_type: "SageMakerTrainingJob",
    name_suffix: "aws-training-job",
};

/// [`TrackedJobKind`] for a processing job.
pub const PROCESSING_JOB_TRACKING: TrackedJobKind = TrackedJobKind {
    source_type: "SageMakerProcessingJob",
    name_suffix: "aws-processing-job",
};

/// [`TrackedJobKind`] for a batch transform job.
pub const TRANSFORM_JOB_TRACKING: TrackedJobKind = TrackedJobKind {
    source_type: "SageMakerTransformJob",
    name_suffix: "aws-transform-job",
};

/// The name SageMaker gives the trial component it creates for a job: the job's own name
/// followed by the suffix of its kind, so the component of one job is identifiable from
/// the job's name alone and two jobs never share one.
pub fn job_trial_component_name(job_name: &str, kind: TrackedJobKind) -> String {
    format!("{job_name}-{}", kind.name_suffix)
}

/// The job a trial component is recorded FOR: its kind, its name and its ARN.
///
/// One parameter rather than three, so the writer of a job's component
/// ([`ComAmazonawsSagemakerSagemakerState::record_job_trial_component`]) takes the job as
/// one thing and no caller can pair one job's name with another's ARN.
#[derive(Debug, Clone, Copy)]
pub struct TrackedJob<'a> {
    /// Which kind of job it is, i.e. how its component is named and sourced.
    pub kind: TrackedJobKind,
    /// The job's own name, which its component's name is built from.
    pub name: &'a str,
    /// The job's ARN, which its component reports as its source.
    pub arn: &'a str,
}

/// The store key one lineage entity of `kind` is held under, and the ARN any resource of
/// `kind` addressed by a bare name OR a full ARN resolves to (the lineage members'
/// `ExperimentEntityNameOrArn` / `ContextNameOrArn`, a `ModelCardNameOrArn`, a
/// `VersionedArnOrName`).
///
/// Every lineage store in this module is keyed by the entity's ARN, which carries both the
/// Region the name is unique within and the name itself — SageMaker documents these names
/// as unique to an account IN A REGION. So a bare name is resolved against `region`, the
/// Region this request signed for, while an ARN is matched as a WHOLE: it addresses the one
/// entity that ARN names, and one differing in the entity kind, the account, the partition
/// or the Region addresses a different entity (and so, here, none). A name can never be
/// mistaken for an ARN: no lineage name pattern admits a `:`.
///
/// Shared by the action slice ([`ComAmazonawsSagemakerSagemakerState::action_key`]), the
/// context slice ([`context_key`]) and the refusals that NAME the resolved ARN
/// ([`model_card_not_found`], [`model_package_not_found`]), so the name-or-ARN split is
/// decided in ONE place: a name has to be resolved, because the caller sent no ARN to
/// report, while an ARN is echoed, because rewriting it would report an identifier the
/// caller never sent.
pub fn arn_from_name_or_arn(region: &str, kind: &str, identifier: &str) -> String {
    if identifier.starts_with(ARN_PREFIX) {
        identifier.to_string()
    } else {
        resource_arn(region, kind, identifier)
    }
}

/// The store key for the context a request addresses: that context's ARN, resolved by
/// [`arn_from_name_or_arn`] against the Region the request signed for.
///
/// `CreateContext`'s `ContextName` is documented unique to an account in a Region, so the
/// name alone does not identify a context — the ARN, which carries the Region and the
/// account beside the name, does. `DescribeContext`'s `ContextNameOrArn` is what lets a
/// caller spell either; `UpdateContext` and `DeleteContext` take a plain `ContextName`,
/// whose pattern admits no `:`, so for them only the name branch is ever reached.
pub fn context_key(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    name_or_arn: &str,
) -> String {
    arn_from_name_or_arn(&request_region(ctx), CONTEXT_KIND, name_or_arn)
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another context fits, i.e. the store is below [`CONTEXT_QUOTA`]. A second
    /// inherent `impl` block, beside this slice's own types rather than in the one above.
    pub fn contexts_below_quota(&self) -> bool {
        self.contexts.len() < CONTEXT_QUOTA
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another trial component fits, i.e. the store is below
    /// [`TRIAL_COMPONENT_QUOTA`]. A second inherent `impl` block, beside this slice's own
    /// types rather than in the one above.
    pub fn trial_components_below_quota(&self) -> bool {
        self.trial_components.len() < TRIAL_COMPONENT_QUOTA
    }

    /// Whether another trial-component membership fits, i.e. [`Self::trial_component_trials`]
    /// is below [`TRIAL_COMPONENT_TRIAL_QUOTA`]. What `AssociateTrialComponent` admits a
    /// NEW membership by.
    pub fn trial_component_trials_below_quota(&self) -> bool {
        self.trial_component_trials.len() < TRIAL_COMPONENT_TRIAL_QUOTA
    }

    /// The `TrialComponentStatus` the trial component `component` reports.
    ///
    /// A component the service created FOR a job REPORTS THAT JOB'S run: its status is the
    /// status of the job its source names, in the same vocabulary — `TrialComponentStatus`'s
    /// `PrimaryStatus` and the three job-status enums share their members (`InProgress`,
    /// `Completed`, `Failed`, `Stopping`, `Stopped`) — so a client polling the component
    /// sees what a client polling the job sees. A component `CreateTrialComponent` made
    /// reports the status that call set on it, and none when it set none.
    ///
    /// A pure read: the job's own status is replayed, never settled here, so reporting a
    /// component changes nothing a later caller sees (the job's own readers materialise the
    /// transitions the clock owes it).
    pub fn trial_component_status(
        &self,
        component: &TrialComponent,
    ) -> Option<crate::ops::types::TrialComponentStatus> {
        let tracked = component
            .source
            .as_ref()
            .and_then(|source| self.tracked_job_status(&source.source_arn));
        match tracked {
            Some(primary_status) => Some(crate::ops::types::TrialComponentStatus {
                primary_status: Some(primary_status),
                // "If the component failed, a message describing why": nothing beside the
                // status is reported for a run this emulator did not measure.
                message: None,
            }),
            None => component.status.clone(),
        }
    }

    /// The status of the job whose ARN is `source_arn`, across the job kinds whose create
    /// carries an `ExperimentConfig` and so can have a trial component of their own.
    ///
    /// `None` when no job of this account has that ARN — a component whose source job has
    /// since been deleted reports no status rather than a stale one.
    pub fn tracked_job_status(&self, source_arn: &str) -> Option<String> {
        self.training_jobs
            .values()
            .find(|job| job.arn == source_arn)
            .map(|job| job.status.clone())
            .or_else(|| {
                self.processing_jobs
                    .values()
                    .find(|job| job.arn == source_arn)
                    .map(|job| job.status.clone())
            })
            .or_else(|| {
                self.transform_jobs
                    .values()
                    .find(|job| job.arn == source_arn)
                    .map(|job| job.status.clone())
            })
    }

    /// The [`entity_key`]s of the trial components whose SOURCE is the job `source_arn`
    /// names, read off the component records themselves.
    ///
    /// What `ListTrialComponents`' `SourceArn` filter selects by: "only components that
    /// have the specified source ARN". A component's source is the job the service created
    /// it for ([`Self::record_job_trial_component`]); one `CreateTrialComponent` made
    /// carries none, so no source ARN selects it.
    pub fn trial_component_keys_with_source(&self, source_arn: &str) -> BTreeSet<String> {
        self.trial_components
            .iter()
            .filter(|(_, component)| {
                component
                    .source
                    .as_ref()
                    .is_some_and(|source| source.source_arn == source_arn)
            })
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// Record the trial component SageMaker creates for a job submitted with an
    /// `ExperimentConfig`, and attach it to the trial that configuration names.
    ///
    /// "Associates a SageMaker job as a trial component with an experiment and trial"
    /// (`ExperimentConfig`'s own doc): the job does not merely remember the configuration
    /// — the service creates a component FOR the job, sourced at the job's ARN, so
    /// `ListTrialComponents(SourceArn=<job-arn>)` reports it and the trial the request
    /// named holds it. This is the ONE place that writes such a component, so every
    /// `ExperimentConfig`-carrying create records one the same way.
    ///
    /// The component's name is [`job_trial_component_name`] and its display name is the
    /// configuration's `TrialComponentDisplayName` when it carries one, following the
    /// "if a display name isn't specified, the name is displayed" rule the rest of this
    /// service's entities state. `Status`, `StartTime` and `EndTime` stay ABSENT: they
    /// report what the job's own run did, which nothing here has measured.
    ///
    /// Answers the ARN of the component this call created, and `None` when it created
    /// none: the component is already there (so a repeated create of the same job name
    /// does not file a second one), or the store is at [`TRIAL_COMPONENT_QUOTA`] and
    /// cannot take another, which bounds the growth a caller varying the job name could
    /// otherwise cause. The at-the-bound case is warned about here, so no caller has to.
    ///
    /// The membership is recorded only where the configuration names a trial this account
    /// HOLDS: `ExperimentConfig$TrialName` names "an existing trial", so a name no trial
    /// answers to attaches the component to nothing rather than inventing a trial.
    pub fn record_job_trial_component(
        &mut self,
        region: &str,
        job: TrackedJob<'_>,
        config: &crate::ops::types::ExperimentConfig,
        created_by: UserContext,
        now: SystemTime,
    ) -> Option<String> {
        let name = job_trial_component_name(job.name, job.kind);
        let key = entity_key(&name);
        if self.trial_components.contains_key(&key) {
            return None;
        }
        if !self.trial_components_below_quota() {
            tracing::warn!(
                trial_component = name,
                bound = TRIAL_COMPONENT_QUOTA,
                "the trial-component store is at its bound, so no component was recorded \
                 for this job"
            );
            return None;
        }

        let arn = resource_arn(region, TRIAL_COMPONENT_KIND, &name);
        // A trial component is a lineage entity, so the Region's lineage group holds it
        // exactly as it holds one `CreateTrialComponent` made.
        self.ensure_lineage_group(region, &default_lineage_group_name(region), now);
        self.trial_components.insert(
            key.clone(),
            TrialComponent {
                name: name.clone(),
                arn: arn.clone(),
                region: region.to_string(),
                display_name: config
                    .trial_component_display_name
                    .clone()
                    .unwrap_or_else(|| name.clone()),
                // What the job's run did is not reported here: nothing has measured a
                // status, a start or an end for it.
                status: None,
                start_time: None,
                end_time: None,
                parameters: HashMap::new(),
                input_artifacts: HashMap::new(),
                output_artifacts: HashMap::new(),
                metadata_properties: None,
                // The identity behind the create that submitted the job: the service made
                // this component on that caller's behalf.
                created_by: created_by.clone(),
                last_modified_by: created_by,
                tags: Vec::new(),
                creation_time: now,
                last_modified_time: now,
                // The job this component tracks, which is what a source-keyed listing
                // and the describe report.
                source: Some(crate::ops::types::TrialComponentSource {
                    source_arn: job.arn.to_string(),
                    source_type: Some(job.kind.source_type.to_string()),
                }),
            },
        );

        // The trial's own stored key, read before the membership index is written so the
        // two borrows do not overlap.
        let trial_key = config
            .trial_name
            .as_deref()
            .and_then(|trial_name| self.trial(trial_name))
            .map(|trial| entity_key(&trial.name));
        if let Some(trial_key) = trial_key {
            let membership = (trial_key, key);
            if self.trial_component_trials_below_quota() {
                self.trial_component_trials.insert(membership);
            } else {
                tracing::warn!(
                    bound = TRIAL_COMPONENT_TRIAL_QUOTA,
                    "the trial-component membership index is at its bound, so this job's \
                     component was not attached to its trial"
                );
            }
        }
        Some(arn)
    }

    /// The [`entity_key`]s of the trial components that are part of the trial `trial_name`
    /// names, read out of [`Self::trial_component_trials`].
    ///
    /// The name is folded the way the trial store folds it, so a filter spelling the trial
    /// in another case selects the same components.
    pub fn trial_component_keys_in_trial(&self, trial_name: &str) -> BTreeSet<String> {
        let trial = entity_key(trial_name);
        self.trial_component_trials
            .iter()
            .filter(|(member_of, _)| member_of == &trial)
            .map(|(_, component)| component.clone())
            .collect()
    }

    /// The [`entity_key`]s of the trials the trial component `component_key` ([`entity_key`]
    /// of its name) is part of, read out of [`Self::trial_component_trials`].
    ///
    /// The other direction of the same index as [`Self::trial_component_keys_in_trial`]:
    /// what `DeleteTrialComponent` tests the documented "must be disassociated from all
    /// trials" precondition by.
    pub fn trial_keys_holding_trial_component(&self, component_key: &str) -> BTreeSet<String> {
        self.trial_component_trials
            .iter()
            .filter(|(_, component)| component == component_key)
            .map(|(member_of, _)| member_of.clone())
            .collect()
    }

    /// The [`entity_key`]s of the trials that belong to the experiment `experiment_name`
    /// names, read out of [`Self::trials`].
    ///
    /// A trial names the experiment it belongs to ([`Trial::experiment_name`]), so the
    /// experiment's membership is read off the trial records rather than kept a second
    /// time beside them. Both names are folded the way the stores fold them, so a caller
    /// spelling the experiment in another case selects the same trials. What
    /// `DeleteExperiment` tests the documented "all trials associated with the experiment
    /// must be deleted first" precondition by, and the first step of
    /// [`Self::trial_component_keys_in_experiment`].
    pub fn trial_keys_in_experiment(&self, experiment_name: &str) -> BTreeSet<String> {
        let experiment = entity_key(experiment_name);
        self.trials
            .values()
            .filter(|trial| entity_key(&trial.experiment_name) == experiment)
            .map(|trial| entity_key(&trial.name))
            .collect()
    }

    /// The [`entity_key`]s of the trial components that are part of ANY trial of the
    /// experiment `experiment_name` names.
    ///
    /// A component is part of an experiment through a trial — a trial names the experiment
    /// it belongs to ([`Trial::experiment_name`]) and a component is attached to a trial —
    /// so this is the membership of the experiment's trials taken together.
    pub fn trial_component_keys_in_experiment(&self, experiment_name: &str) -> BTreeSet<String> {
        let trials = self.trial_keys_in_experiment(experiment_name);
        self.trial_component_trials
            .iter()
            .filter(|(member_of, _)| trials.contains(member_of))
            .map(|(_, component)| component.clone())
            .collect()
    }
}

/// The store key a `TrialComponentName` (or any other case-insensitive SageMaker name,
/// such as an `AutoMLJobName`) maps to.
///
/// The member's model doc says the name "must be unique in your Amazon Web Services
/// account and is not case-sensitive" (`AutoMLJobName`'s says "must be unique to your
/// account and is case insensitive"), so two names differing only in case are ONE
/// entity: the store is keyed by the case-folded name, while the record keeps the casing
/// the caller sent so a reader replays it. Every such name is ASCII (`ExperimentEntityName`
/// is `^[a-zA-Z0-9](-*[a-zA-Z0-9]){0,119}$`, `AutoMLJobName` the same shape bounded at 32),
/// so ASCII folding is exact for every name the boundary validator lets through.
pub fn entity_key(name: &str) -> String {
    name.to_ascii_lowercase()
}

/// The entity NAME a member modelled `ExperimentEntityNameOrArn` denotes, or `None` when
/// the value is an ARN that denotes something else.
///
/// Those members accept either the bare name or the entity's ARN, so a reader resolves
/// both forms to one store key rather than keying the store by the string the caller
/// typed. An ARN denotes an entity of THIS service only when it is an ARN this service
/// mints: `arn:aws:sagemaker:<the request's region>:<the emulator's account>:<kind>/<name>`.
/// An ARN naming another service, account, region or resource kind names no entity here —
/// the caller gets the reader's not-found error rather than whatever entity happens to
/// share the ARN's trailing path segment. That validation is why this is stricter than
/// [`name_from_arn_or_name`], which serves the `ArnOrName` readers whose pattern admits
/// one resource kind only: `ExperimentEntityNameOrArn` also admits the ARNs of OTHER
/// entity kinds (experiment, trial, artifact, action, context), so the kind has to be
/// checked rather than assumed.
pub fn entity_name_from_name_or_arn<'v>(
    value: &'v str,
    region: &str,
    kind: &str,
) -> Option<&'v str> {
    if !value.starts_with("arn:") {
        return Some(value);
    }
    // The fixed segments are checked once, by `arn_resource`.
    let name = arn_resource(value, region)?
        .strip_prefix(kind)?
        .strip_prefix('/')?;
    (!name.is_empty() && !name.contains('/')).then_some(name)
}
