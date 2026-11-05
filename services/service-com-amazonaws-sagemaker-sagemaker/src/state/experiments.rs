// SPDX-License-Identifier: CC0-1.0

//! The EXPERIMENT-TRACKING slice of this service's state: the trial components
//! `CreateTrialComponent` writes, the experiments that hold them, and the identifier
//! resolution every one of their operations shares.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit. Every name is re-exported from [`super`], so a handler keeps
//! naming `crate::state::…`.

use super::*;

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

/// The ARN resource kind an experiment carries, as the modelled `ExperimentArn` pattern
/// spells it (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:experiment/`).
///
/// Named once, the way the lineage slices name theirs, so `CreateExperiment`'s ARN
/// minting and the ARN PARSING that resolves an `experiment/...` ARN back to a stored
/// record cannot drift apart.
pub const EXPERIMENT_KIND: &str = "experiment";

/// See [`MODEL_QUOTA`]. Bounds the experiment store the same way, and is what
/// `CreateExperiment` answers `ResourceLimitExceeded` past.
pub const EXPERIMENT_QUOTA: usize = 1_000;

/// An experiment created by `CreateExperiment`, as `DescribeExperiment` replays it.
///
/// `name` keeps the spelling the creating request used; the store's key is
/// [`entity_key`] of that name, so a later create or read that differs only in case
/// resolves to this same record. `created_by` is stored at creation rather than rebuilt
/// per read, so every later read reports the identity that actually created it, and
/// `last_modified_by` starts as that same identity — the creation IS the last
/// modification until `UpdateExperiment` records the identity behind its own call, the
/// way the context slice's does.
#[derive(Debug, Clone)]
pub struct Experiment {
    pub name: String,
    pub arn: String,
    /// What `DescribeExperiment` reports as `DisplayName`: the value the request
    /// supplied, or `ExperimentName` when it supplied none — the model documents that
    /// "if DisplayName isn't specified, ExperimentName is displayed".
    pub display_name: String,
    pub description: Option<String>,
    pub created_by: UserContext,
    /// Who last modified the experiment: the creating identity until a call modifies it.
    pub last_modified_by: UserContext,
    /// The tags the create carried, which `AddTags`/`DeleteTags`/`ListTags` and `Search`
    /// read once those operations are implemented (they are scaffolded stubs today).
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another experiment fits, i.e. the store is below [`EXPERIMENT_QUOTA`].
    pub fn experiments_below_quota(&self) -> bool {
        self.experiments.len() < EXPERIMENT_QUOTA
    }

    /// The experiment `name` identifies, whatever case it is written in — the name is
    /// unique per account and not case-sensitive, so the lookup folds case the way
    /// [`entity_key`] does.
    pub fn experiment(&self, name: &str) -> Option<&Experiment> {
        self.experiments.get(&entity_key(name))
    }
}
