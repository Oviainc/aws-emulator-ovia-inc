// SPDX-License-Identifier: CC0-1.0

//! The experiment-management slice of this service's state: the [`Experiment`] record,
//! its bound and ARN kind, and the lookup its readers share.

use super::*;

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
