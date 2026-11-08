// SPDX-License-Identifier: CC0-1.0

//! The Amazon Personalize PROVISIONING pipeline: the records a dataset import job, a
//! solution, a solution version, a campaign and a filter persist, the ARNs they are
//! addressed by, the recipes a solution may be trained with, and the output shapes each
//! `Describe*` rebuilds from its record.
//!
//! # Why these live together
//!
//! `CreateDatasetImportJob` → `CreateSolution` → `CreateSolutionVersion` →
//! `CreateCampaign` is ONE pipeline: each stage reads the stage before it, and the
//! campaign at the end is what the Amazon Personalize Runtime inference plane serves.
//! The rule every stage follows is stated once here rather than per handler:
//!
//! * a resource is addressed by an ARN minted from its own identity (its name, or its
//!   solution and version ordinal), so a duplicate create collides on an ARN that is
//!   already present;
//! * a create answers before its work is complete, and the status a `Describe*` reports
//!   is a function of the stored due instant and the clock
//!   ([`crate::state::terminal_status`]) — a read reports a transition the clock has
//!   already made due and writes nothing;
//! * a stage that depends on an earlier one becomes ready only once that one is
//!   ([`due_after`]), so a campaign is never served before the solution version it
//!   deploys has finished training;
//! * the work itself is REAL: an import reads object storage and a solution version is
//!   trained on the records that import (or the ingestion data plane) actually stored,
//!   so a stage whose work could not be carried out ends in `CREATE FAILED` carrying the
//!   reason instead of reporting a success nothing produced.

use std::time::SystemTime;

use ::service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::state as runtime;

use crate::ops::types::{
    Campaign, CampaignConfig, DataSource, DatasetImportJob, Filter, Solution, SolutionConfig,
    SolutionVersion, SolutionVersionSummary, Tag,
};
use crate::state::{active_at, creation_status, terminal_status, timestamp_of, STATUS_ACTIVE};

// ---------------------------------------------------------------------------
// Recipes
// ---------------------------------------------------------------------------

/// One Amazon Personalize recipe: the ARN a `CreateSolution` names it by, the
/// `recipeType` it belongs to, which runtime operation serves a campaign trained with
/// it, and whether it can be retrained incrementally.
#[derive(Debug, Clone, Copy)]
pub struct RecipeEntry {
    /// The recipe's ARN, as `CreateSolution$recipeArn` names it.
    pub arn: &'static str,
    /// The recipe type, in the spelling Amazon Personalize reports for a recipe.
    pub recipe_type: &'static str,
    /// Which runtime operation serves a campaign deploying a solution version trained
    /// with this recipe, or `None` for a recipe whose results are read in BATCH (a
    /// user-segmentation recipe produces segments, not real-time recommendations) and
    /// which therefore cannot be deployed in a campaign at all.
    pub served_by: Option<runtime::RecipeType>,
    /// Whether a solution version of this recipe may be trained in `UPDATE` mode: "Only
    /// solution versions created with the User-Personalization recipe can use UPDATE"
    /// (`SolutionVersion$trainingMode` doc).
    pub supports_update: bool,
}

/// The recipes this emulator knows, each with the recipe type that decides which
/// inference operation serves a campaign on it.
///
/// Hand-written catalog. Source: the recipe types the runtime operations' own
/// documentation names — `USER_PERSONALIZATION` and `RELATED_ITEMS` for
/// `GetRecommendations`, `PERSONALIZED_RANKING` for `GetPersonalizedRanking` ("The
/// solution backing the campaign must have been created using a recipe of type
/// PERSONALIZED_RANKING") and `PERSONALIZED_ACTIONS` for `GetActionRecommendations` —
/// paired with the recipe ARNs the Amazon Personalize "Choosing a recipe" documentation
/// publishes under `arn:aws:personalize:::recipe/`. The model carries no enumeration of
/// recipe ARNs (`recipeArn` is a plain `Arn` string), so the set cannot come from it.
///
/// An ARN that is not here names no recipe this emulator holds, and a `CreateSolution`
/// that asks for one is refused where the recipe is resolved rather than left to fail at
/// inference time.
pub const RECIPES: &[RecipeEntry] = &[
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-user-personalization",
        recipe_type: "USER_PERSONALIZATION",
        served_by: Some(runtime::RecipeType::UserPersonalization),
        supports_update: true,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-user-personalization-v2",
        recipe_type: "USER_PERSONALIZATION",
        served_by: Some(runtime::RecipeType::UserPersonalization),
        supports_update: true,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-popularity-count",
        recipe_type: "USER_PERSONALIZATION",
        served_by: Some(runtime::RecipeType::UserPersonalization),
        supports_update: false,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-trending-now",
        recipe_type: "USER_PERSONALIZATION",
        served_by: Some(runtime::RecipeType::UserPersonalization),
        supports_update: false,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-similar-items",
        recipe_type: "RELATED_ITEMS",
        served_by: Some(runtime::RecipeType::RelatedItems),
        supports_update: false,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-sims",
        recipe_type: "RELATED_ITEMS",
        served_by: Some(runtime::RecipeType::RelatedItems),
        supports_update: false,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-personalized-ranking",
        recipe_type: "PERSONALIZED_RANKING",
        served_by: Some(runtime::RecipeType::PersonalizedRanking),
        supports_update: false,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-personalized-ranking-v2",
        recipe_type: "PERSONALIZED_RANKING",
        served_by: Some(runtime::RecipeType::PersonalizedRanking),
        supports_update: false,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-next-best-action",
        recipe_type: "PERSONALIZED_ACTIONS",
        served_by: Some(runtime::RecipeType::PersonalizedActions),
        supports_update: false,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-item-affinity",
        recipe_type: "USER_SEGMENTATION",
        served_by: None,
        supports_update: false,
    },
    RecipeEntry {
        arn: "arn:aws:personalize:::recipe/aws-user-segmentation",
        recipe_type: "USER_SEGMENTATION",
        served_by: None,
        supports_update: false,
    },
];

/// The recipe `arn` names, or `None` when it names no recipe at all.
pub fn recipe(arn: &str) -> Option<&'static RecipeEntry> {
    RECIPES.iter().find(|entry| entry.arn == arn)
}

/// The recipe a `performAutoML` solution is trained with, or `None` when the search has
/// no candidate at all (a `recipeList` naming none of this emulator's
/// `USER_PERSONALIZATION` recipes).
///
/// "When true, Amazon Personalize performs a search for the best USER_PERSONALIZATION
/// recipe from the list specified in the solution configuration"
/// (`CreateSolution$performAutoML`), and what the search settled on is reported in
/// `Solution$autoMLResult`. This emulator runs no search: the recipe it reports is the
/// first `USER_PERSONALIZATION` entry of [`RECIPES`] the request admits, which is the
/// recipe Amazon Personalize publishes as the current User-Personalization one.
pub fn auto_ml_recipe(candidates: Option<&[String]>) -> Option<&'static RecipeEntry> {
    // "The AutoMLConfig object containing a list of recipes to search when AutoML is
    // performed" — a named candidate list narrows the search to the recipes it holds.
    RECIPES.iter().find(|entry| {
        entry.recipe_type == AUTO_ML_RECIPE_TYPE
            && candidates.is_none_or(|candidates| {
                candidates.is_empty() || candidates.iter().any(|candidate| candidate == entry.arn)
            })
    })
}

/// The recipe type an AutoML search chooses from: "Amazon Personalize performs a search
/// for the best USER_PERSONALIZATION recipe" (`CreateSolution$performAutoML`).
pub const AUTO_ML_RECIPE_TYPE: &str = "USER_PERSONALIZATION";

// ---------------------------------------------------------------------------
// ARNs
// ---------------------------------------------------------------------------

/// A dataset import job's ARN:
/// `arn:aws:personalize:<region>:<account>:dataset-import-job/<jobName>`.
pub fn dataset_import_job_arn(region: &str, job_name: &str) -> String {
    ::emulator_server::emulator_arn(
        "personalize",
        region,
        &format!("dataset-import-job/{job_name}"),
    )
}

/// A solution's ARN: `arn:aws:personalize:<region>:<account>:solution/<name>`.
pub fn solution_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("personalize", region, &format!("solution/{name}"))
}

/// A solution version's ARN:
/// `arn:aws:personalize:<region>:<account>:solution/<solution name>/<version id>`.
///
/// A solution version has no name of its own that identifies it — `name` is optional and
/// not unique — so its identity inside the solution is the ORDINAL of its create, which
/// [`version_id`] renders as the fixed-width hexadecimal id Amazon Personalize appends to
/// the solution's own ARN.
pub fn solution_version_arn(region: &str, solution_name: &str, ordinal: u32) -> String {
    ::emulator_server::emulator_arn(
        "personalize",
        region,
        &format!("solution/{solution_name}/{}", version_id(ordinal)),
    )
}

/// The id of the `ordinal`-th solution version of one solution, as the last segment of
/// its ARN: eight hexadecimal digits, so it is unique per solution and reproducible
/// rather than per run.
pub fn version_id(ordinal: u32) -> String {
    format!("{ordinal:08x}")
}

/// A campaign's ARN: `arn:aws:personalize:<region>:<account>:campaign/<name>`.
pub fn campaign_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("personalize", region, &format!("campaign/{name}"))
}

/// A filter's ARN: `arn:aws:personalize:<region>:<account>:filter/<name>`.
pub fn filter_arn(region: &str, name: &str) -> String {
    ::emulator_server::emulator_arn("personalize", region, &format!("filter/{name}"))
}

// ---------------------------------------------------------------------------
// The clock, for a stage that waits on an earlier one
// ---------------------------------------------------------------------------

/// The instant a resource created now is due, when it cannot be ready before
/// `dependency` is.
///
/// A campaign deploys a solution version and a solution version is trained on a
/// solution's dataset group, so a stage created while the stage below it is still
/// pending takes its own span AFTER that one completes: a composite resource reaches its
/// terminal status only once what it depends on has.
pub fn due_after(created_at: SystemTime, dependency: SystemTime) -> SystemTime {
    active_at(created_at.max(dependency))
}

// ---------------------------------------------------------------------------
// Records
// ---------------------------------------------------------------------------

/// What `CreateDatasetImportJob` persists, and what `DescribeDatasetImportJob`
/// reconstructs a [`DatasetImportJob`] from.
#[derive(Debug, Clone)]
pub struct DatasetImportJobRecord {
    pub job_name: String,
    pub dataset_import_job_arn: String,
    pub dataset_arn: String,
    /// The `dataLocation` the job read, exactly as the caller supplied it.
    pub data_location: Option<String>,
    pub role_arn: Option<String>,
    pub import_mode: String,
    pub publish_attribution_metrics_to_s3: Option<bool>,
    /// How many records the job actually imported into its dataset.
    pub imported_rows: usize,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    /// The instant the clock owes this job its terminal status.
    pub active_at: SystemTime,
    /// Why the import could not be carried out, for a job that ends `CREATE FAILED`.
    pub failure_reason: Option<String>,
    pub tags: Vec<Tag>,
}

/// What `CreateSolution` persists, and what `DescribeSolution` reconstructs a
/// [`Solution`] from.
#[derive(Debug, Clone)]
pub struct SolutionRecord {
    pub name: String,
    pub solution_arn: String,
    pub dataset_group_arn: String,
    /// The region the dataset group's ARN is scoped to, so every resource of one
    /// pipeline is minted in the same region.
    pub region: String,
    pub perform_hpo: bool,
    pub perform_auto_ml: bool,
    pub perform_auto_training: Option<bool>,
    pub perform_incremental_update: Option<bool>,
    /// The recipe the solution trains with: the `recipeArn` the request named, or the
    /// one AutoML settled on ([`auto_ml_recipe`]).
    pub recipe_arn: String,
    /// Whether that recipe was chosen by AutoML, which is what makes it the
    /// `autoMLResult` a describe reports.
    pub auto_ml_result: bool,
    pub event_type: Option<String>,
    pub solution_config: Option<SolutionConfig>,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    pub active_at: SystemTime,
    /// How many solution versions have been created from this solution, which is what
    /// the next one's version id is minted from.
    pub versions_created: u32,
    pub tags: Vec<Tag>,
}

/// What `CreateSolutionVersion` persists, and what `DescribeSolutionVersion`
/// reconstructs a [`SolutionVersion`] from.
///
/// The TRAINED model is [`Self::trained`]: the catalog the version was fitted to, read
/// out of the dataset group's own records when the version was created. A campaign
/// deploying this version recommends from exactly that catalog.
#[derive(Debug, Clone)]
pub struct SolutionVersionRecord {
    pub name: Option<String>,
    pub solution_version_arn: String,
    pub solution_arn: String,
    pub dataset_group_arn: String,
    pub recipe_arn: String,
    pub perform_hpo: bool,
    pub perform_auto_ml: bool,
    pub perform_incremental_update: Option<bool>,
    pub event_type: Option<String>,
    pub solution_config: Option<SolutionConfig>,
    pub training_mode: String,
    pub training_type: String,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    pub active_at: SystemTime,
    /// Why training could not be carried out, for a version that ends `CREATE FAILED`.
    pub failure_reason: Option<String>,
    /// The catalog this version was trained on.
    pub trained: crate::state::training::TrainedCatalog,
    pub tags: Vec<Tag>,
}

/// What `CreateCampaign` persists, and what `DescribeCampaign` reconstructs a
/// [`Campaign`] from.
///
/// The campaign the INFERENCE plane serves is the one `CreateCampaign` registered in the
/// Amazon Personalize Runtime registry; this record is the control plane's own view of
/// it, and both carry the same due instant, so what `DescribeCampaign` reports ACTIVE is
/// exactly what the runtime serves.
#[derive(Debug, Clone)]
pub struct CampaignRecord {
    pub name: String,
    pub campaign_arn: String,
    pub solution_version_arn: String,
    pub min_provisioned_tps: i32,
    pub campaign_config: Option<CampaignConfig>,
    /// How many catalog entries this campaign's copy in the inference registry holds:
    /// what it costs the engine, measured against [`MAX_RETAINED_CATALOG_ENTRIES`].
    pub catalog_entries: usize,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    pub active_at: SystemTime,
    pub tags: Vec<Tag>,
}

/// What `CreateFilter` persists, and what `DescribeFilter` reconstructs a [`Filter`]
/// from. The parsed expression itself lives in the runtime registry, which is what
/// applies it.
#[derive(Debug, Clone)]
pub struct FilterRecord {
    pub name: String,
    pub filter_arn: String,
    pub dataset_group_arn: String,
    pub filter_expression: String,
    pub creation_date_time: SystemTime,
    pub last_updated_date_time: SystemTime,
    pub active_at: SystemTime,
    pub tags: Vec<Tag>,
}

/// "The default import mode is FULL" (`CreateDatasetImportJob$importMode`).
pub const DEFAULT_IMPORT_MODE: &str = "FULL";
/// The import mode that "append[s] the new records to the existing data in your
/// dataset" (`CreateDatasetImportJob$importMode`).
pub const INCREMENTAL_IMPORT_MODE: &str = "INCREMENTAL";

/// The default `trainingMode`: "A FULL training considers all of the data in your
/// dataset group" (`SolutionVersion$trainingMode`).
pub const DEFAULT_TRAINING_MODE: &str = "FULL";
/// The `trainingMode` that "processes only the data that has changed since the latest
/// training" (`SolutionVersion$trainingMode`).
pub const UPDATE_TRAINING_MODE: &str = "UPDATE";

/// `TrainingType` for a version a caller asked for, as opposed to one auto training
/// created: "Whether the solution version was created automatically or manually"
/// (`SolutionVersion$trainingType`).
pub const MANUAL_TRAINING_TYPE: &str = "MANUAL";

/// "We recommend starting with 1 for minProvisionedTPS (the default)"
/// (`Campaign$minProvisionedTPS`).
pub const DEFAULT_MIN_PROVISIONED_TPS: i32 = 1;

/// The most catalog entries this store keeps resident across every trained solution
/// version, and (counted on its own) across every deployed campaign.
///
/// An EMULATOR bound on the PRODUCT the two per-resource ceilings would otherwise leave
/// open: a trained version keeps the catalog it was fitted to
/// ([`crate::state::training::MAX_CATALOG_ENTRIES`]) and a campaign deploying it keeps a
/// copy in the inference registry, so without this, `MAX_SOLUTION_VERSIONS` versions of
/// `MAX_CATALOG_ENTRIES` entries each would be resident at once. Checked BEFORE a
/// version is recorded and before a campaign is registered, so a create past it is
/// refused with the modelled `LimitExceededException` instead of allocating.
pub const MAX_RETAINED_CATALOG_ENTRIES: usize = 50_000;

// ---------------------------------------------------------------------------
// Output shapes, rebuilt from the records
// ---------------------------------------------------------------------------

/// The `DatasetImportJob` `DescribeDatasetImportJob` returns for a stored record.
pub fn dataset_import_job_of(record: &DatasetImportJobRecord) -> DatasetImportJob {
    let status = terminal_status(record.active_at, record.failure_reason.as_deref());
    DatasetImportJob {
        job_name: Some(record.job_name.clone()),
        dataset_import_job_arn: Some(record.dataset_import_job_arn.clone()),
        dataset_arn: Some(record.dataset_arn.clone()),
        data_source: Some(DataSource {
            data_location: record.data_location.clone(),
        }),
        role_arn: record.role_arn.clone(),
        status: Some(status.to_string()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        // "If a dataset import job fails, provides the reason why" — reported only once
        // the clock has made the failure due, so a job still in progress carries none.
        failure_reason: record
            .failure_reason
            .clone()
            .filter(|_| status == crate::state::STATUS_CREATE_FAILED),
        import_mode: Some(record.import_mode.clone()),
        publish_attribution_metrics_to_s3: record.publish_attribution_metrics_to_s3,
    }
}

/// The `Solution` `DescribeSolution` returns for a stored record, with the latest of its
/// `versions` summarised the way `Solution$latestSolutionVersion` documents ("Describes
/// the latest version of the solution, including the status and the ARN").
pub fn solution_of(record: &SolutionRecord, versions: &[&SolutionVersionRecord]) -> Solution {
    // The LATEST version: the most recently created one, with the ARN settling a pair
    // created in the same instant so the answer is stable across reads.
    let latest: Option<&SolutionVersionRecord> = versions.iter().copied().max_by(|left, right| {
        (left.creation_date_time, &left.solution_version_arn)
            .cmp(&(right.creation_date_time, &right.solution_version_arn))
    });
    Solution {
        name: Some(record.name.clone()),
        solution_arn: Some(record.solution_arn.clone()),
        perform_hpo: record.perform_hpo,
        perform_auto_ml: record.perform_auto_ml,
        perform_auto_training: record.perform_auto_training,
        perform_incremental_update: record.perform_incremental_update,
        recipe_arn: Some(record.recipe_arn.clone()),
        dataset_group_arn: Some(record.dataset_group_arn.clone()),
        event_type: record.event_type.clone(),
        solution_config: record.solution_config.clone(),
        // "When performAutoML is true, specifies the best recipe found" — so the member
        // is present only for a solution whose recipe AutoML chose.
        auto_ml_result: record.auto_ml_result.then(|| crate::ops::types::AutoMLResult {
            best_recipe_arn: Some(record.recipe_arn.clone()),
        }),
        status: Some(creation_status(record.active_at).to_string()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        latest_solution_version: latest.map(solution_version_summary_of),
        // `UpdateSolution` is not implemented, so no solution has been updated and there
        // is no latest update to describe.
        latest_solution_update: None,
    }
}

/// The `SolutionVersionSummary` a solution's `latestSolutionVersion` carries.
pub fn solution_version_summary_of(record: &SolutionVersionRecord) -> SolutionVersionSummary {
    let status = terminal_status(record.active_at, record.failure_reason.as_deref());
    SolutionVersionSummary {
        solution_version_arn: Some(record.solution_version_arn.clone()),
        status: Some(status.to_string()),
        training_mode: Some(record.training_mode.clone()),
        training_type: Some(record.training_type.clone()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        failure_reason: record
            .failure_reason
            .clone()
            .filter(|_| status == crate::state::STATUS_CREATE_FAILED),
    }
}

/// The `SolutionVersion` `DescribeSolutionVersion` returns for a stored record.
pub fn solution_version_of(record: &SolutionVersionRecord) -> SolutionVersion {
    let status = terminal_status(record.active_at, record.failure_reason.as_deref());
    SolutionVersion {
        name: record.name.clone(),
        solution_version_arn: Some(record.solution_version_arn.clone()),
        solution_arn: Some(record.solution_arn.clone()),
        perform_hpo: record.perform_hpo,
        perform_auto_ml: record.perform_auto_ml,
        perform_incremental_update: record.perform_incremental_update,
        recipe_arn: Some(record.recipe_arn.clone()),
        event_type: record.event_type.clone(),
        dataset_group_arn: Some(record.dataset_group_arn.clone()),
        solution_config: record.solution_config.clone(),
        // "The time used to train the model [...] visible only after Amazon Personalize
        // successfully trains a model": this emulator fits its model from the stored
        // records in the create itself, so there are no billed training hours to report
        // and the member stays absent rather than invented.
        training_hours: None,
        training_mode: Some(record.training_mode.clone()),
        // Reported only when hyperparameter optimization actually ran, which it does
        // not here, so no tuned values are invented.
        tuned_hpo_params: None,
        status: Some(status.to_string()),
        failure_reason: record
            .failure_reason
            .clone()
            .filter(|_| status == crate::state::STATUS_CREATE_FAILED),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        training_type: Some(record.training_type.clone()),
    }
}

/// The `Campaign` `DescribeCampaign` returns for a stored record.
pub fn campaign_of(record: &CampaignRecord) -> Campaign {
    Campaign {
        name: Some(record.name.clone()),
        campaign_arn: Some(record.campaign_arn.clone()),
        solution_version_arn: Some(record.solution_version_arn.clone()),
        min_provisioned_tps: Some(record.min_provisioned_tps),
        campaign_config: record.campaign_config.clone(),
        status: Some(creation_status(record.active_at).to_string()),
        // Nothing fails a campaign create asynchronously: a solution version that could
        // not be deployed is refused at the submission.
        failure_reason: None,
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        // "The latestCampaignUpdate field is only returned when the campaign has had at
        // least one UpdateCampaign call."
        latest_campaign_update: None,
    }
}

/// The `Filter` `DescribeFilter` returns for a stored record.
pub fn filter_of(record: &FilterRecord) -> Filter {
    Filter {
        name: Some(record.name.clone()),
        filter_arn: Some(record.filter_arn.clone()),
        creation_date_time: Some(timestamp_of(record.creation_date_time)),
        last_updated_date_time: Some(timestamp_of(record.last_updated_date_time)),
        dataset_group_arn: Some(record.dataset_group_arn.clone()),
        // An expression the runtime could not apply is refused by `CreateFilter`, so a
        // stored filter never failed.
        failure_reason: None,
        filter_expression: Some(record.filter_expression.clone()),
        status: Some(creation_status(record.active_at).to_string()),
    }
}

// ---------------------------------------------------------------------------
// Refusal wording
// ---------------------------------------------------------------------------

/// What a create refused because the resource it needs has not finished its own create
/// answers: the ARN, the status it is in, and the status it has to reach.
///
/// One sentence for the whole service, so `CreateDatasetImportJob` and `CreateCampaign`
/// word the in-use class the same way `CreateDataset` words it for a dataset group.
pub fn not_active_yet_message(kind: &str, arn: &str, status: &str) -> String {
    format!("{kind} {arn} is in status {status}; wait until it is {STATUS_ACTIVE}.")
}

/// What a create refused because a name is taken answers: the kind of resource and the
/// name the request asked for.
pub fn already_exists_message(kind: &str, name: &str) -> String {
    format!("A {kind} with the name {name} already exists.")
}

/// What a create refused because this emulator holds as many of a kind as it keeps
/// answers. The bound is the EMULATOR's own ceiling (see [`crate::state::MAX_SCHEMAS`]).
pub fn limit_reached_message(kind: &str, capacity: usize) -> String {
    format!("This emulator holds at most {capacity} {kind}; delete one before creating another.")
}

/// What a create refused because the store already keeps as many catalog entries as it
/// retains answers: the bound, and the kind of resource that would have added to it.
pub fn catalog_limit_reached_message(kind: &str) -> String {
    format!(
        "This emulator keeps at most {MAX_RETAINED_CATALOG_ENTRIES} trained catalog entries \
         across every {kind}; delete one before creating another."
    )
}
