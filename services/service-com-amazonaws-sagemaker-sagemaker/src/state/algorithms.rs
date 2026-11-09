// SPDX-License-Identifier: CC0-1.0

//! The algorithm slice of this service's state: the records `CreateAlgorithm`,
//! `DescribeAlgorithm` and `DeleteAlgorithm` keep.
//!
//! It lives in its own module file, re-exported from [`super`] so handlers keep naming
//! `crate::state::...`, because the parent module is a single large file every new
//! resource slice appends to; a slice of its own keeps this resource's constants, record
//! and store guard together and out of those shared regions.

use std::time::{Duration, SystemTime};

use super::{ComAmazonawsSagemakerSagemakerState, STATUS_COMPLETED};
use crate::ops::types::{
    AlgorithmStatusDetails, AlgorithmStatusItem, AlgorithmValidationSpecification,
    InferenceSpecification, Tag, TrainingSpecification,
};

/// The most algorithms this service's store holds. See [`super::MODEL_QUOTA`].
pub const ALGORITHM_QUOTA: usize = 1_000;

/// The ARN resource kind an algorithm carries, i.e. the `<kind>` in
/// [`super::resource_arn`], as the modelled `AlgorithmArn` pattern spells it
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]{9,16}:[0-9]{12}:algorithm/[\S]{1,2048}$`).
/// Named once here so the create mints, the readers resolve and the refusals match the
/// one kind.
pub const ALGORITHM_ARN_KIND: &str = "algorithm";

/// How long after `CreateAlgorithm` the clock owes the algorithm's transition out of
/// `Pending`. A caller that describes the algorithm straight after creating it still
/// observes `Pending`; one that waits observes the terminal status. Real SageMaker takes
/// minutes; the span is compressed, never zero.
pub const ALGORITHM_COMPLETED_AFTER: Duration = Duration::from_millis(300);

/// `AlgorithmStatus`'s non-terminal member a new algorithm is born in (the modelled enum
/// also carries `InProgress` and `Deleting`, which this slice does not reach: a delete is
/// immediate). Named once so no handler compares a bare string literal.
pub const ALGORITHM_STATUS_PENDING: &str = "Pending";

/// `AlgorithmStatus`'s terminal failure member, and the `DetailedAlgorithmStatus` of the
/// validation entries that failed with it (both modelled enums spell it `Failed`).
pub const ALGORITHM_STATUS_FAILED: &str = "Failed";

/// The `DetailedAlgorithmStatus` an algorithm's per-profile validation entry carries
/// before the clock's transition falls due: nothing has run it yet.
pub const DETAILED_ALGORITHM_STATUS_NOT_STARTED: &str = "NotStarted";

/// Why an algorithm whose create asked for validation ends in
/// [`ALGORITHM_STATUS_FAILED`].
///
/// A `ValidationSpecification` asks SageMaker to run validation training and transform
/// jobs; this emulator runs none, so the validation cannot be performed. The honest
/// answer is the service's own modelled failure state with a reason that says so,
/// rather than a `Completed` no work produced or a `Pending` that never resolves.
pub const VALIDATION_NOT_RUN: &str = "This emulator runs no algorithm validation training \
     or transform jobs, so the validation this algorithm requested was not performed.";

/// Why a delete of an algorithm that has not reached a terminal status is refused,
/// naming the status that stands in the way.
///
/// An algorithm is registered asynchronously: while it is still `Pending` SageMaker is
/// listing it and (when the create asked for one) running its validation, so there is
/// nothing settled to remove and the delete is refused rather than destroying an
/// algorithm mid-registration. The operation models `ConflictException` only for a
/// collision with a concurrent modification, so this refusal is a 400
/// `ValidationException` carrying this sentence.
pub fn algorithm_not_deletable(status: &str) -> String {
    format!("Cannot delete Algorithm in {status} status.")
}

/// An algorithm created by `CreateAlgorithm`, as `DescribeAlgorithm` replays it.
///
/// `completed_at` is the instant the clock owes the algorithm's transition out of
/// `Pending`; it is stored, never slept on, and materialised by whichever reporting
/// call first runs at or after it.
///
/// `tags` is persisted because `CreateAlgorithm` accepts it, and is not read back here:
/// `DescribeAlgorithm` models no `Tags` member, and the tag-read operations own that
/// surface. It is stored so those readers find what the create was given rather than a
/// second copy of the resource.
#[derive(Debug, Clone)]
pub struct Algorithm {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    pub training_specification: TrainingSpecification,
    pub inference_specification: Option<InferenceSpecification>,
    pub validation_specification: Option<AlgorithmValidationSpecification>,
    pub certify_for_marketplace: Option<bool>,
    pub status: String,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub completed_at: SystemTime,
}

impl Algorithm {
    /// Materialise the transition the clock already owes: `Pending` -> `Completed` for
    /// an algorithm with nothing to validate, `Pending` -> `Failed` for one whose
    /// create asked for validation jobs this emulator does not run.
    ///
    /// This is the one write a reporting operation may make: the transition was due at
    /// `completed_at` whether or not anybody called, so the observer makes no
    /// difference to the result. Either way the algorithm lands in a TERMINAL status, so
    /// a caller never polls a status that cannot resolve; the failure carries
    /// [`VALIDATION_NOT_RUN`] on every validation entry, so what was not run is stated
    /// rather than reported as a success.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == ALGORITHM_STATUS_PENDING && now >= self.completed_at {
            self.status = if self.validation_specification.is_some() {
                ALGORITHM_STATUS_FAILED.to_string()
            } else {
                STATUS_COMPLETED.to_string()
            };
            return true;
        }
        false
    }

    /// Whether the algorithm has reached a status nothing leaves on its own, i.e. its
    /// registration has settled — [`STATUS_COMPLETED`] for an algorithm with nothing to
    /// validate, [`ALGORITHM_STATUS_FAILED`] for one whose create asked for validation
    /// jobs this emulator does not run. Only a settled algorithm can be deleted; see
    /// [`algorithm_not_deletable`].
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status.as_str(),
            STATUS_COMPLETED | ALGORITHM_STATUS_FAILED
        )
    }

    /// The algorithm's `AlgorithmStatusDetails`, derived from what it stores.
    ///
    /// One `ValidationStatuses` entry per `ValidationProfiles` profile, and an empty
    /// list when the create asked for no validation. An entry is
    /// [`DETAILED_ALGORITHM_STATUS_NOT_STARTED`] while the algorithm is still `Pending`
    /// and [`ALGORITHM_STATUS_FAILED`] with [`VALIDATION_NOT_RUN`] once it has settled,
    /// because no validation job ran. `ImageScanStatuses` is always empty: the emulator
    /// scans no container image, so it has no scan to report.
    pub fn status_details(&self) -> AlgorithmStatusDetails {
        let settled = self.status != ALGORITHM_STATUS_PENDING;
        let validation_statuses = self
            .validation_specification
            .as_ref()
            .map(|specification| {
                specification
                    .validation_profiles
                    .iter()
                    .map(|profile| AlgorithmStatusItem {
                        name: profile.profile_name.clone(),
                        status: if settled {
                            ALGORITHM_STATUS_FAILED.to_string()
                        } else {
                            DETAILED_ALGORITHM_STATUS_NOT_STARTED.to_string()
                        },
                        failure_reason: settled.then(|| VALIDATION_NOT_RUN.to_string()),
                    })
                    .collect()
            })
            .unwrap_or_default();
        AlgorithmStatusDetails {
            validation_statuses: Some(validation_statuses),
            image_scan_statuses: Some(Vec::new()),
        }
    }
}

/// The algorithm store's own guard, beside the record it bounds rather than in the state
/// struct's main `impl` block, for the reason this module's docs give.
impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another algorithm fits, i.e. the store is below [`ALGORITHM_QUOTA`].
    pub fn algorithms_below_quota(&self) -> bool {
        self.algorithms.len() < ALGORITHM_QUOTA
    }
}
