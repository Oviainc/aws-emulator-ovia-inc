// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-databrew-awsgluedatabrew service.
//!
//! DataBrew addresses datasets and jobs by NAME (scoped to the caller's
//! account/region), and a job run by its job's name plus the minted run id, so the
//! maps below are keyed exactly that way. Everything an operation answers with is
//! computed from these records — the handlers hold no state of their own.
//!
//! Besides the records this module carries the few derivations several handlers
//! share (the caller's principal and region, a resource ARN, the `Source` a stored
//! `Input` implies, name validation and page tokens). They live here rather than in
//! one operation's file because `src/lib.rs` is generated and cannot declare another
//! module, and because every one of them is a property of the stored record.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    DataCatalogOutput, Dataset, DatabaseOutput, FormatOptions, Input, JobRun, Output, PathOptions,
    Recipe, RecipeReference, RecipeStep, Sample,
};

/// What a recipe job RUN does: reads the dataset's object, applies the recipe version's
/// steps and writes the result to the job's outputs.
pub mod job_execution;
/// The transformations a recipe's steps name, applied to a read table.
pub mod recipe_steps;
/// The tabular value a dataset's object holds and a job's output is written from.
pub mod tabular_data;

pub use recipe_steps::recipe_step_error_message;

/// The resource kinds DataBrew states a refusal about in its SHORT form: the kind, the
/// identifier the request carried and the bare predicate, with no terminating period
/// (`Schedule nightly not found`, `Schedule nightly already exists`). Every other kind
/// is stated in the service's sentence form (`Dataset orders wasn't found.`,
/// `Dataset orders already exists.`).
///
/// The wording belongs to the RESOURCE the request addressed rather than to the
/// operation that addressed it, so one kind reads the same from every lookup and every
/// create that names it, and an operation added on a kind listed here states what the
/// kind's existing operations state without a case of its own.
const SHORT_FORM_RESOURCE_KINDS: [&str; 1] = ["Schedule"];

/// Whether DataBrew states refusals about `kind` in its short, period-less form
/// ([`SHORT_FORM_RESOURCE_KINDS`]).
fn stated_in_short_form(kind: &str) -> bool {
    SHORT_FORM_RESOURCE_KINDS.contains(&kind)
}

/// The resource kinds whose NOT-FOUND sentence DataBrew words with the definite
/// article and the kind spelled in LOWER case (`The project orders wasn't found.`),
/// rather than with the capitalised bare kind the sentence form otherwise takes
/// (`Dataset orders wasn't found.`).
///
/// Like [`SHORT_FORM_RESOURCE_KINDS`] this wording belongs to the RESOURCE the lookup
/// addressed rather than to the operation that addressed it, so a kind listed here
/// reads identically from every operation that resolves it by name and an operation
/// added on such a kind needs no case of its own. A recipe addressed together with a
/// version is worded this way too, in the pair sentence
/// [`recipe_version_not_found_message`] builds.
const DEFINITE_ARTICLE_NOT_FOUND_KINDS: [&str; 1] = ["Project"];

/// Whether DataBrew words `kind`'s not-found sentence with the definite article and the
/// kind in lower case ([`DEFINITE_ARTICLE_NOT_FOUND_KINDS`]).
fn not_found_stated_with_article(kind: &str) -> bool {
    DEFINITE_ARTICLE_NOT_FOUND_KINDS.contains(&kind)
}

/// The message DataBrew answers a `ResourceNotFoundException` with: the kind of
/// resource the lookup addressed, the identifier the request carried for it, and the
/// predicate stating it was not found.
///
/// The sentence the model documents on the error SHAPE ("One or more resources can't
/// be found.") describes the error class, not the message a particular lookup
/// answers with: DataBrew builds that message from the request, so a client can tell
/// which resource was missing, and of which kind. `kind` is the resource kind as
/// DataBrew names it in prose, capitalised (`Dataset`, `Job`, `Recipe`, `Schedule`);
/// `identifier` is what the request looked the resource up by. The sentence is the one
/// DataBrew words that kind's refusals with: the short, period-less predicate for the
/// kinds listed in [`SHORT_FORM_RESOURCE_KINDS`], the definite-article sentence for
/// those in [`DEFINITE_ARTICLE_NOT_FOUND_KINDS`], and the capitalised bare-kind
/// sentence for every other kind.
pub fn not_found_message(kind: &str, identifier: &str) -> String {
    if stated_in_short_form(kind) {
        format!("{kind} {identifier} not found")
    } else if not_found_stated_with_article(kind) {
        format!("The {} {identifier} wasn't found.", kind.to_lowercase())
    } else {
        format!("{kind} {identifier} wasn't found.")
    }
}

/// The message a lookup that addressed a (recipe, VERSION) pair answers its
/// `ResourceNotFoundException` with: the recipe name the request carried and the
/// version identifier that was looked up, which is a different sentence from the
/// single-resource one [`not_found_message`] builds.
///
/// A recipe is versioned, so an operation that takes a `RecipeVersion` never looks a
/// recipe up on its own: it resolves the pair, and the message names both halves so a
/// client can tell "no such recipe" from "that recipe holds no such version".
pub fn recipe_version_not_found_message(name: &str, version: &str) -> String {
    format!("The recipe {name} for version {version} wasn't found.")
}

/// The `ErrorMessage` a PER-ITEM version failure reports for a version the recipe does
/// not hold: the recipe name the request carried and the version identifier that item
/// asked for.
///
/// A batch version delete reports each version's failure as an item of its own rather
/// than refusing the whole request, and the item's sentence states the pair it could
/// not resolve the same way a single lookup does — but in the per-item wording recorded
/// for this operation, which names the version without the `for` of
/// [`recipe_version_not_found_message`].
pub fn recipe_version_item_not_found_message(name: &str, version: &str) -> String {
    format!("The recipe {name} version {version} wasn't found.")
}

/// The `ErrorMessage` a per-item version failure reports for a version a PROJECT is
/// developing: the recipe the request addressed, the version identifier the item asked
/// for, and the project that holds it, read from state.
///
/// A project develops its recipe's working version, so that version cannot be deleted
/// while the project exists; the sentence names the (recipe, version) pair that is held
/// and then the project holding it, so a client reading a 200 with partial failures can
/// tell WHICH recipe version survived and what to delete first.
pub fn recipe_version_used_by_project_message(name: &str, version: &str, project: &str) -> String {
    recipe_version_used_by_message(name, version, "project", project)
}

/// The `ErrorMessage` a per-item version failure reports for a version a JOB runs: the
/// recipe the request addressed, the version identifier the item asked for, and the job
/// whose recipe reference names it.
///
/// A job runs one version of one recipe (model: `Job$RecipeReference`), so a version a
/// job references cannot be deleted out from under it. The sentence is the one
/// [`recipe_version_used_by_project_message`] states, for a holder that is a job.
pub fn recipe_version_used_by_job_message(name: &str, version: &str, job: &str) -> String {
    recipe_version_used_by_message(name, version, "job", job)
}

/// The one sentence every "this recipe version is still held" refusal states: the recipe
/// name and the version identifier the request addressed, the kind of resource holding
/// that version, and its name — every part read from the request or from state.
fn recipe_version_used_by_message(
    name: &str,
    version: &str,
    holder_kind: &str,
    holder: &str,
) -> String {
    format!("Recipe {name} version {version} are used by {holder_kind} {holder}.")
}

/// The message `DeleteRecipeVersion` refuses the working draft with: the operation does
/// not delete that version, stated as DataBrew's own sentence about the identifier this
/// operation will not take.
///
/// The sentence has NO variable part — it names neither the recipe, nor the versions the
/// recipe holds, nor the number the draft currently carries; the alias in it is the
/// literal identifier the refusal is about, not a value echoed from the request. The
/// single-version delete states that this version is not deletable through it, where the
/// per-item failure of the batch operation states the ORDERING rule instead
/// ([`working_version_not_last_item_message`]), so the two operations keep saying
/// different things about the same version.
pub fn working_version_delete_refused_message() -> String {
    format!("Recipe version {LATEST_WORKING} is not allowed to be deleted")
}

/// The `ErrorMessage` a per-item version failure of the BATCH delete reports for the
/// working draft of a recipe that still holds other versions: the recipe name the request
/// addressed and the version identifier that item asked for, then the ordering rule.
///
/// "The LATEST_WORKING version will only be deleted if the recipe has no other versions"
/// (model: `BatchDeleteRecipeVersion`), so the batch operation reports the draft as a
/// partial failure that states WHAT clears it — delete the other versions and ask again —
/// and names the pair it is about, since one request's `Errors` can carry several items.
pub fn working_version_not_last_item_message(name: &str, version: &str) -> String {
    format!(
        "Recipe {name} version {version} can only be deleted if all other versions got \
         deleted."
    )
}

/// The message a schedule write answers its refusal with when a name in `JobNames`
/// resolves to no job: the fixed `Job ` / ` not found` text, with the name the request
/// carried, so a client can tell WHICH of the jobs it named does not exist.
///
/// A schedule runs the jobs it names, so a name that resolves to no job is reported as
/// the miss it is, in DataBrew's `<resource> <identifier> not found` phrasing, rather
/// than as a nameless complaint about the member.
pub fn job_not_found_message(name: &str) -> String {
    format!("Job {name} not found")
}

/// The message DataBrew answers a `ConflictException` with when a create names a
/// resource that already exists: the resource kind, capitalised, and the name the
/// request carried, so the message states which resource conflicted. The predicate is
/// the one DataBrew words that kind's refusals with ([`SHORT_FORM_RESOURCE_KINDS`]).
pub fn already_exists_message(kind: &str, name: &str) -> String {
    if stated_in_short_form(kind) {
        format!("{kind} {name} already exists")
    } else {
        format!("{kind} {name} already exists.")
    }
}

/// The message `CreateProject` answers its `ConflictException` with when the recipe the
/// create names is already developed in another project: DataBrew's own sentence for
/// this refusal, which names the recipe the request carried, the ONE project that
/// already holds it, and the step that clears the refusal.
///
/// The sentence the model documents on the error SHAPE ("Updating or deleting a
/// resource can cause an inconsistent state.") describes the error class, and the error
/// shape carries no per-resource member, so the message is the only detail a client can
/// tell this 409 by. The project is named in the SINGULAR because a recipe belongs to at
/// most one project (model: `Recipe$ProjectName` is a single name), so there is never
/// more than one holder to report, and the remediation clause states the rule the caller
/// ran into: a recipe may be part of only one project.
pub fn recipe_already_in_project_message(recipe_name: &str, project_name: &str) -> String {
    format!(
        "The recipe {recipe_name} is already associated with project {project_name}. \
         Choose a recipe that isn't already part of a project."
    )
}

/// The message `DeleteDataset` answers its `ConflictException` with when a project was
/// created against the dataset the request addresses: DataBrew's own per-resource
/// sentence, which names the dataset, the ONE project that acts upon it, and the step
/// that clears the refusal.
///
/// The sentence the model documents on the error SHAPE ("Updating or deleting a
/// resource can cause an inconsistent state.") describes the error class, and the
/// message is the only detail this error carries about which resource pinned which —
/// there is no per-resource member on the error shape — so a client that tells this 409
/// from any other 409 reads the message. The project is named in the SINGULAR because
/// DataBrew's sentence is: a dataset several projects act upon reports the first of
/// them in name order, and the delete runs into the same refusal for the next one until
/// every project naming the dataset is gone.
pub fn dataset_in_use_by_project_message(dataset_name: &str, project_name: &str) -> String {
    format!(
        "The dataset {dataset_name} is used in project {project_name}. To delete this \
         dataset, first delete the associated project."
    )
}

/// The region an ARN carries when the request's SigV4 scope names none.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The service principal a role must let assume it to be a role DataBrew runs a job as.
///
/// A job's `RoleArn` is "the IAM role to be assumed when DataBrew runs the job" (model:
/// `CreateRecipeJobRequest$RoleArn`), and the principal that assumes it is this service.
pub const SERVICE_PRINCIPAL: &str = "databrew.amazonaws.com";

/// The message DataBrew answers its `ValidationException` with for a job whose data
/// access role does not let this service assume it: the service's own sentence, with the
/// role ARN the REQUEST carried as its variable part, so a client can tell which value
/// was refused.
///
/// The sentence the model documents on the error SHAPE ("The input parameters for this
/// request failed validation.") describes the error class rather than this refusal, and
/// the error carries no per-member field, so the message is the only detail a client has.
pub fn untrusted_role_message(role_arn: &str) -> String {
    format!("DataBrew is not a trusted entity for the data access role {role_arn}.")
}

/// The message DataBrew answers its `ValidationException` with when a job create selects
/// no output at all: every modelled output list (`Outputs`, `DataCatalogOutputs`,
/// `DatabaseOutputs`) is absent, so the job has nowhere to write and DataBrew refuses it
/// rather than creating a job whose run can produce nothing.
pub const NO_OUTPUTS_SELECTED_MESSAGE: &str = "No outputs were selected for this job.";

/// The `MaxCapacity` a job carries when the create asked for none.
///
/// `MaxCapacity` is "the maximum number of nodes that DataBrew can consume when the job
/// processes data" (model: `CreateRecipeJobRequest$MaxCapacity`, whose wire default is
/// 0 — "no value given"): a job runs on nodes, so DataBrew fills in its own default
/// rather than running a job with none, and that is the capacity a later `DescribeJob`
/// reports.
pub const DEFAULT_MAX_CAPACITY: i32 = 5;

/// The `MaxCapacity` a created job carries: the one the request asked for, else
/// [`DEFAULT_MAX_CAPACITY`]. A request that carries no value sends the modelled default
/// of 0, which is not a number of nodes a job can run on.
pub fn effective_max_capacity(requested: i32) -> i32 {
    if requested > 0 {
        requested
    } else {
        DEFAULT_MAX_CAPACITY
    }
}

/// Whether this emulator's own IAM ANSWERED that `role_arn` is a role DataBrew cannot
/// assume: this account holds a role under the name the ARN carries, and the trust policy
/// that role carries does not admit [`SERVICE_PRINCIPAL`].
///
/// A job RUNS AS its `RoleArn`, so a role DataBrew cannot assume backs no run at all and
/// the service settles that before it stores the job. The whole question — which role of
/// this account the value names, what IAM answers about it, and whether the trust document
/// it carries admits a service principal — is the ONE reading every emulated service that
/// resolves a caller-supplied execution role shares
/// (`service_common::iam_role::held_role_is_unassumable`), so this crate states only WHICH
/// principal has to be admitted and WHICH operations ask. IAM is reached the way any
/// emulated service reaches another: an AWS SDK client over the emulator's own in-process
/// callback, so nothing here opens a socket or reads an ambient credential.
///
/// `false` — do not refuse — wherever nothing is KNOWN about a role of this account: a
/// value naming no role of this account, an emulator with no IAM mounted to ask, an answer
/// this emulator could not read, and a role this account's IAM holds nothing under. The
/// last case is why the reading is the one that SERVES such a request: a data access role
/// is routinely provisioned out of band, so "IAM holds no such role" is evidence about
/// this emulator's store rather than about the role the caller named. A refusal has to be
/// grounded in the trust document IAM itself returned. Awaited with NO store guard held.
pub async fn role_is_unassumable(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> bool {
    ::service_common::iam_role::held_role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL)
        .await
}

/// `MaxResults100`'s modelled maximum, used as the page size when a request asks
/// for none (model: `ListDatasets`/`ListJobRuns` `maxResults` range 1..100).
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most datasets this emulator retains for one account/region.
///
/// Every create is a growth site a caller drives, so the store carries an explicit
/// ceiling: past it the create is REFUSED with the modelled
/// `ServiceQuotaExceededException` (the error AWS answers a quota with) rather than
/// inserting, so emulator memory cannot be grown without bound.
pub const MAX_DATASETS: usize = 1_000;

/// The most jobs this emulator retains, for the reason [`MAX_DATASETS`] gives.
pub const MAX_JOBS: usize = 1_000;

/// The most schedules this emulator retains, for the reason [`MAX_DATASETS`] gives.
pub const MAX_SCHEDULES: usize = 1_000;

/// The most recipes this emulator retains, for the reason [`MAX_DATASETS`] gives.
pub const MAX_RECIPES: usize = 1_000;

/// The most projects this emulator retains, for the reason [`MAX_DATASETS`] gives.
pub const MAX_PROJECTS: usize = 1_000;

/// The most published versions this emulator retains for one recipe.
///
/// Every publish freezes another version of the recipe, so it is a growth site a
/// caller drives: past this ceiling the publish is REFUSED with the modelled
/// `ServiceQuotaExceededException` (the error AWS answers a quota with) rather than
/// appending, for the reason [`MAX_DATASETS`] gives.
pub const MAX_PUBLISHED_RECIPE_VERSIONS: usize = 1_000;

/// The number of rows a project samples when the create named no `Sample`: "If this
/// parameter isn't specified, then the sample consists of the first 500 rows from the
/// dataset" (model: `Project$Sample`).
pub const DEFAULT_SAMPLE_SIZE: i32 = 500;

/// The sampling type that same sentence describes — the FIRST rows of the dataset
/// (model: `SampleType` is one of `FIRST_N`, `LAST_N`, `RANDOM`).
pub const DEFAULT_SAMPLE_TYPE: &str = "FIRST_N";

/// The version identifier a recipe carries the moment `CreateRecipe` mints it.
///
/// A recipe version is `X.Y`, major and minor (model: `Recipe$RecipeVersion` —
/// "Numeric version (X.Y) ... Both X and Y are required, and \"0.0\" isn't a valid
/// version"), and publishing takes the next MAJOR number, so the version a recipe is
/// created at is the first minor of major 0: its working version, which no publish has
/// frozen yet.
pub const INITIAL_RECIPE_VERSION: &str = "0.1";

/// The alias naming "the most recent valid version being developed" (model:
/// `Recipe$RecipeVersion`).
pub const LATEST_WORKING: &str = "LATEST_WORKING";

/// The alias naming "the most recent published version" (model:
/// `Recipe$RecipeVersion`), and the identifier a request that carries no
/// `RecipeVersion` asks for: "If this parameter isn't specified, then the latest
/// published version is returned" (model: `DescribeRecipeRequest$RecipeVersion`).
pub const LATEST_PUBLISHED: &str = "LATEST_PUBLISHED";

/// The version identifier a request looked a recipe version up by: the one it carried,
/// or — when it carried none — the alias an omitted identifier asks for.
///
/// An omitted `RecipeVersion` is not "any version this recipe holds": the model
/// documents it as asking for the latest published version, so it is exactly
/// [`LATEST_PUBLISHED`] that resolves, and the same identifier is what a miss reports.
pub fn requested_recipe_version(requested: Option<&str>) -> &str {
    requested.unwrap_or(LATEST_PUBLISHED)
}

/// The most digits one half of a numeric recipe version may carry: "The maximum
/// length of each is 6 digits" (model: `Recipe$RecipeVersion`).
const RECIPE_VERSION_MAX_DIGITS: usize = 6;

/// The most runs this emulator retains for one job. `StartJobRun` inserts a record
/// on every call and a run has no name of its own to collide on, so retention is
/// bounded by EVICTING the oldest run of the job rather than by refusing the start:
/// a run history is a rolling window, and the newest runs are the ones a caller
/// polls.
pub const MAX_RUNS_PER_JOB: usize = 100;

/// The most runs this emulator retains across every job, evicted oldest-first for
/// the reason [`MAX_RUNS_PER_JOB`] gives.
pub const MAX_JOB_RUNS: usize = 10_000;

/// How long after `StartJobRun` a run's `STARTING -> RUNNING` transition falls due.
///
/// A DataBrew run is returned in a non-terminal state and reaches a terminal one on
/// its own, so the emulator owes the transition to the clock rather than to a later
/// call. The span is chosen (AWS publishes no duration for it) and deliberately
/// non-zero, so a caller that describes the run straight after starting it still
/// observes `STARTING`.
const RUN_RUNNING_DUE_AFTER: Duration = Duration::from_millis(300);

/// How long after `StartJobRun` a run's terminal `SUCCEEDED` transition falls due.
/// Chosen for the same reason as [`RUN_RUNNING_DUE_AFTER`].
const RUN_TERMINAL_DUE_AFTER: Duration = Duration::from_millis(600);

/// The create/modify audit a named DataBrew resource carries, as the modelled
/// `CreateDate`/`CreatedBy`/`LastModifiedDate`/`LastModifiedBy` members report it.
///
/// The create pair is set once, when the resource is minted. The last-modified pair
/// is `None` until an operation that MODIFIES the resource runs: the modelled members
/// are optional (model: only `Name` and `Input` are required on
/// `DescribeDatasetResponse`), and a resource that has never been modified has no
/// modification to report, so both members are absent from the response. That is what
/// lets a client tell a freshly created resource from an edited one.
#[derive(Debug, Clone)]
pub struct Audit {
    pub create_date: SystemTime,
    pub created_by: String,
    pub last_modified_date: Option<SystemTime>,
    pub last_modified_by: Option<String>,
}

impl Audit {
    /// The audit of a resource `principal` created at `at`: created, never modified.
    pub fn created(at: SystemTime, principal: String) -> Self {
        Self {
            create_date: at,
            created_by: principal,
            last_modified_date: None,
            last_modified_by: None,
        }
    }

    /// Record that `principal` modified the resource at `at`. Called ONLY from an
    /// operation the spec says mutates the resource — never from a read.
    pub fn modified(&mut self, at: SystemTime, principal: String) {
        self.last_modified_date = Some(at);
        self.last_modified_by = Some(principal);
    }

    /// `LastModifiedDate` as the wire member: absent while the resource is unmodified.
    pub fn wire_last_modified_date(&self) -> Option<::aws_smithy_types::DateTime> {
        self.last_modified_date.map(wire_date)
    }

    /// `LastModifiedBy` as the wire member: absent while the resource is unmodified.
    pub fn wire_last_modified_by(&self) -> Option<String> {
        self.last_modified_by.clone()
    }
}

/// A stored DataBrew dataset. `Source`, the ARN and the [`Audit`] are server-derived,
/// never client-supplied.
#[derive(Debug, Clone)]
pub struct DatasetRecord {
    pub name: String,
    pub format: Option<String>,
    pub format_options: Option<FormatOptions>,
    pub input: Input,
    pub path_options: Option<PathOptions>,
    pub source: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub resource_arn: String,
    pub audit: Audit,
}

/// A stored DataBrew job. `job_type` is the modelled `JobType` (`RECIPE` for a job
/// created by `CreateRecipeJob`, `PROFILE` by `CreateProfileJob`).
#[derive(Debug, Clone)]
pub struct JobRecord {
    pub name: String,
    pub job_type: String,
    pub dataset_name: Option<String>,
    pub project_name: Option<String>,
    pub recipe_reference: Option<RecipeReference>,
    pub outputs: Vec<Output>,
    /// The Glue Data Catalog outputs the create selected, held so a read reports what the
    /// job was created with. A recipe job run writes an Amazon S3 output; these are
    /// reported, never written ([`job_execution::plan`]).
    pub data_catalog_outputs: Vec<DataCatalogOutput>,
    /// The JDBC database outputs the create selected, held for the reason
    /// [`JobRecord::data_catalog_outputs`] gives.
    pub database_outputs: Vec<DatabaseOutput>,
    pub role_arn: String,
    pub max_capacity: i32,
    pub max_retries: i32,
    pub timeout: i32,
    pub encryption_mode: Option<String>,
    pub encryption_key_arn: Option<String>,
    pub log_subscription: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub resource_arn: String,
    pub audit: Audit,
}

/// A stored DataBrew schedule: the cron expression it runs on and the jobs it runs.
/// The ARN and the [`Audit`] are server-derived, never client-supplied.
#[derive(Debug, Clone)]
pub struct ScheduleRecord {
    pub name: String,
    pub cron_expression: String,
    pub job_names: Vec<String>,
    pub tags: BTreeMap<String, String>,
    pub resource_arn: String,
    pub audit: Audit,
}

/// One version of a stored recipe: the definition that version holds, plus who
/// created, last modified and published it.
///
/// A recipe is versioned (model: `Recipe$RecipeVersion`), so the definition members —
/// the steps and the description — belong to the VERSION rather than to the recipe:
/// publishing freezes a version while development carries on from the working one, so
/// two versions of one recipe answer with two different definitions.
#[derive(Debug, Clone)]
pub struct RecipeVersionRecord {
    pub version: String,
    pub description: Option<String>,
    pub steps: Vec<RecipeStep>,
    /// Set only on a version a publish froze; `None` on a working version, whose
    /// modelled `PublishedBy`/`PublishedDate` report a publish that has not happened
    /// ([`RecipeRecord::publish`] is what sets them).
    pub published_by: Option<String>,
    pub published_date: Option<SystemTime>,
    pub audit: Audit,
}

/// A stored DataBrew recipe: the versions it holds, plus the members that belong to
/// the recipe as a whole (its name, ARN, tags and the project it is attached to).
///
/// A recipe is created holding one working version and publishing adds frozen versions
/// beside it, so the two are held apart rather than in one map: `LATEST_WORKING` and
/// `LATEST_PUBLISHED` name exactly these. Every recipe has a working version for as long
/// as the recipe exists: that version is deleted only when the recipe holds no other
/// version, and the delete that takes it takes the recipe with it.
#[derive(Debug, Clone)]
pub struct RecipeRecord {
    pub name: String,
    /// The project this recipe is attached to, which is a project's doing rather than
    /// a recipe's: `None` until a project names the recipe (`CreateProject`, still its
    /// own stub issue, is what sets it).
    pub project_name: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub resource_arn: String,
    /// The `LATEST_WORKING` version — the one development continues on. Every recipe
    /// has one.
    pub working: RecipeVersionRecord,
    /// The published versions, oldest first, so the last of them is
    /// `LATEST_PUBLISHED`. Empty until a publish freezes a version.
    pub published: Vec<RecipeVersionRecord>,
}

/// A stored DataBrew project: the dataset it acts upon, the recipe developed in it,
/// the role DataBrew assumes for it and the sample it reads. The ARN and the [`Audit`]
/// are server-derived, never client-supplied.
///
/// The session members are held apart from the definition because a session is opened
/// by a LATER call rather than by the create: a project nobody has opened reports no
/// `SessionStatus`, `OpenedBy` or `OpenDate` (model: all three optional on
/// `DescribeProjectResponse`), which is what lets a client tell an idle project from
/// one with an interactive session.
#[derive(Debug, Clone)]
pub struct ProjectRecord {
    pub name: String,
    pub dataset_name: String,
    pub recipe_name: String,
    pub role_arn: String,
    /// The sample the create asked for, `None` when it asked for none — in which case
    /// the project reads the documented default, which [`ProjectRecord::effective_sample`]
    /// is.
    pub sample: Option<Sample>,
    pub tags: BTreeMap<String, String>,
    pub resource_arn: String,
    pub audit: Audit,
    /// The state of this project's interactive session, `None` until a session is
    /// opened for it (`StartProjectSession`, still its own stub issue, is what opens
    /// one).
    pub session_status: Option<String>,
    pub opened_by: Option<String>,
    pub open_date: Option<SystemTime>,
}

impl ProjectRecord {
    /// The sample this project reads with: the one the create carried, with the
    /// documented default size filled in for a half it left out, and the documented
    /// default sample entirely when the create carried none.
    ///
    /// "If this parameter isn't specified, then the sample consists of the first 500
    /// rows from the dataset" (model: `Project$Sample`) — so an omitted sample still
    /// samples, and so does a sample that names only its type: the size it reads is
    /// the documented one. `Sample$Type` is required, so a sample always carries it.
    /// The default is a property of the project rather than of one read, which is why
    /// it is derived here instead of being stored as if the caller had asked for it.
    pub fn effective_sample(&self) -> Sample {
        match &self.sample {
            Some(sample) => Sample {
                size: sample.size.or(Some(DEFAULT_SAMPLE_SIZE)),
                r#type: sample.r#type.clone(),
            },
            None => Sample {
                size: Some(DEFAULT_SAMPLE_SIZE),
                r#type: DEFAULT_SAMPLE_TYPE.to_string(),
            },
        }
    }
}

/// A stored run of a job: what `StartJobRun` minted plus the job's shape copied at
/// start time, so the run still reports what it ran with after the job changes.
#[derive(Debug, Clone)]
pub struct JobRunRecord {
    pub run_id: String,
    pub job_name: String,
    pub dataset_name: Option<String>,
    pub state: String,
    pub attempt: i32,
    pub execution_time: i32,
    pub started_on: SystemTime,
    pub started_by: String,
    pub completed_on: Option<SystemTime>,
    pub error_message: Option<String>,
    pub outputs: Vec<Output>,
    /// The other outputs the job was created with, copied onto the run for the reason
    /// every other member of the job's shape is ([`JobRecord::data_catalog_outputs`]).
    pub data_catalog_outputs: Vec<DataCatalogOutput>,
    pub database_outputs: Vec<DatabaseOutput>,
    pub recipe_reference: Option<RecipeReference>,
    pub log_subscription: Option<String>,
    pub log_group_name: Option<String>,
    /// What the run's WORK answered, recorded when the run was performed and revealed
    /// by [`JobRunRecord::advance_to`] once the terminal transition falls due.
    ///
    /// A run is reported non-terminal first and reaches its terminal state on the clock,
    /// but the state it reaches is not the clock's to choose: it is whatever reading the
    /// dataset, applying the recipe and writing the outputs answered
    /// ([`job_execution::perform`]). Holding the two apart is what keeps a `SUCCEEDED`
    /// run backed by the objects the run wrote.
    pub terminal_outcome: Option<RunOutcome>,
}

/// What performing a job run answered: the terminal `JobRunState` it reached and, for a
/// run that did not finish its work, the `ErrorMessage` that says why.
#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub state: String,
    pub error_message: Option<String>,
}

impl RunOutcome {
    /// The outcome of a run that read its dataset, applied its recipe and wrote every
    /// output.
    pub fn succeeded() -> Self {
        Self {
            state: "SUCCEEDED".to_string(),
            error_message: None,
        }
    }

    /// The outcome of a run that did not write its output, carrying the reason as the
    /// modelled `ErrorMessage`.
    pub fn failed(reason: String) -> Self {
        Self {
            state: "FAILED".to_string(),
            error_message: Some(reason),
        }
    }
}

/// The `ErrorMessage` a run reports when its work never reported an outcome at all —
/// the handler that started it did not get to the end (the client went away, the
/// process was shutting down). Nothing ran, so nothing may be reported as succeeded.
pub const UNPERFORMED_RUN_MESSAGE: &str =
    "The job run did not complete: the emulator did not finish performing it.";

/// Per-service state for com-amazonaws-databrew-awsgluedatabrew. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDatabrewAwsgluedatabrewState {
    /// Datasets by `Name`.
    pub datasets: BTreeMap<String, DatasetRecord>,
    /// Recipe and profile jobs by `Name`.
    pub jobs: BTreeMap<String, JobRecord>,
    /// Schedules by `Name`.
    pub schedules: BTreeMap<String, ScheduleRecord>,
    /// Recipes by `Name`, each holding its own versions.
    pub recipes: BTreeMap<String, RecipeRecord>,
    /// Projects by `Name`.
    pub projects: BTreeMap<String, ProjectRecord>,
    /// Job runs by `(JobName, RunId)`, so one job's runs are a contiguous range.
    pub job_runs: BTreeMap<(String, String), JobRunRecord>,
    /// Monotonic source of the `RunId` values stored in `job_runs`.
    pub next_run_seq: u64,
}

impl ComAmazonawsDatabrewAwsgluedatabrewState {
    /// Mint the next run id. Monotonic and unique within this emulator, and the id
    /// the run is stored under, so a `DescribeJobRun` with it resolves.
    pub fn mint_run_id(&mut self) -> String {
        self.next_run_seq += 1;
        format!("db_{:024}", self.next_run_seq)
    }

    /// Store a run, then evict the oldest runs over [`MAX_RUNS_PER_JOB`] for its job
    /// and over [`MAX_JOB_RUNS`] in total, so a caller that keeps starting runs grows
    /// the store to a fixed ceiling and no further.
    ///
    /// Run ids are minted monotonically and zero-padded, so the LOWEST id of a job is
    /// its oldest run and the lowest id overall is the oldest run held.
    pub fn record_run(&mut self, record: JobRunRecord) {
        let job_name = record.job_name.clone();
        self.job_runs
            .insert((job_name.clone(), record.run_id.clone()), record);

        while self.runs_held_for(&job_name) > MAX_RUNS_PER_JOB {
            let Some(oldest) = self.oldest_run_of(&job_name) else {
                break;
            };
            self.job_runs.remove(&oldest);
        }
        while self.job_runs.len() > MAX_JOB_RUNS {
            let Some(oldest) = self
                .job_runs
                .keys()
                .min_by(|left, right| left.1.cmp(&right.1))
                .cloned()
            else {
                break;
            };
            self.job_runs.remove(&oldest);
        }
    }

    /// How many runs of `job_name` the store holds.
    fn runs_held_for(&self, job_name: &str) -> usize {
        self.job_runs.keys().filter(|key| key.0 == job_name).count()
    }

    /// The key of the oldest retained run of `job_name`; the map is ordered by
    /// `(JobName, RunId)`, so that is the first key of the job's range.
    fn oldest_run_of(&self, job_name: &str) -> Option<(String, String)> {
        self.job_runs
            .keys()
            .find(|key| key.0 == job_name)
            .cloned()
    }

    /// One page of the runs of `job_name`, in run-id order, materialising the
    /// transitions the clock already owed the runs ON THIS PAGE.
    ///
    /// A reporting operation may materialise a transition the clock already made due
    /// — it was owed at this instant whether or not anybody called. The walk is a
    /// `BTreeMap` range that stops at the end of the page, so neither the work nor
    /// the allocation is sized by the store.
    pub fn page_runs_of_job(
        &mut self,
        job_name: &str,
        resume_after: Option<&str>,
        page_size: usize,
        now: SystemTime,
    ) -> Page<JobRun> {
        let lower = match resume_after {
            Some(run_id) => Bound::Excluded((job_name.to_string(), run_id.to_string())),
            None => Bound::Included((job_name.to_string(), String::new())),
        };

        let mut items = Vec::new();
        let mut last_key: Option<String> = None;
        let mut more = false;
        for (key, run) in self.job_runs.range_mut((lower, Bound::Unbounded)) {
            // The key orders every run of one job before the next job's, so the
            // first key of another job ends this job's range.
            if key.0 != job_name {
                break;
            }
            if items.len() == page_size {
                more = true;
                break;
            }
            run.advance_to(now);
            items.push(run.to_wire());
            last_key = Some(key.1.clone());
        }

        Page {
            items,
            next_token: match (more, last_key) {
                (true, Some(key)) => Some(encode_page_token(&key)),
                _ => None,
            },
        }
    }

    /// The job that runs a version of `recipe_name` addressed by any of `identifiers`,
    /// or `None` when no stored job references that version.
    ///
    /// A job runs ONE version of one recipe (model: `Job$RecipeReference`), so a version
    /// a job references cannot be deleted out from under it, and every operation that
    /// deletes a recipe version asks this the same way. One version is addressed by more
    /// than one identifier — its own number and the alias that resolves to it — so the
    /// caller passes every identifier that names the version it is asking about; a
    /// reference carrying no version addresses the latest published one exactly as a
    /// versionless lookup does. The store is keyed by job name, so the job answered is
    /// the first in that order and two calls about one version always name the same job.
    pub fn job_running_recipe_version(
        &self,
        recipe_name: &str,
        identifiers: &[&str],
    ) -> Option<String> {
        self.jobs
            .values()
            .find(|job| {
                job.recipe_reference.as_ref().is_some_and(|reference| {
                    reference.name == recipe_name
                        && identifiers.contains(
                            &reference
                                .recipe_version
                                .as_deref()
                                .unwrap_or(LATEST_PUBLISHED),
                        )
                })
            })
            .map(|job| job.name.clone())
    }
}

impl DatasetRecord {
    /// Apply an `UpdateDataset` request's definition onto this record and stamp the
    /// audit's last-modified pair, so a later read serves what the update wrote.
    ///
    /// `UpdateDataset` is a PUT that carries the dataset's whole definition (model:
    /// `UpdateDataset` `PUT /datasets/{Name}` with `Input` required), so each
    /// definition member is REPLACED by what the request carries: an optional member
    /// the request omits is cleared, not kept. `Source` is re-derived from the new
    /// `Input`, and `Name`, the ARN, the tags and the create pair are not the
    /// operation's to change.
    pub fn apply_update(
        &mut self,
        format: Option<String>,
        format_options: Option<FormatOptions>,
        input: Input,
        path_options: Option<PathOptions>,
        at: SystemTime,
        principal: String,
    ) {
        self.source = source_of(&input).map(str::to_string);
        self.format = format;
        self.format_options = format_options;
        self.input = input;
        self.path_options = path_options;
        self.audit.modified(at, principal);
    }

    /// This record as the modelled `Dataset` that `ListDatasets` returns.
    pub fn to_wire(&self) -> Dataset {
        Dataset {
            account_id: Some(::engine::emulator_account_id().to_string()),
            created_by: Some(self.audit.created_by.clone()),
            create_date: Some(wire_date(self.audit.create_date)),
            name: self.name.clone(),
            format: self.format.clone(),
            format_options: self.format_options.clone(),
            input: self.input.clone(),
            last_modified_date: self.audit.wire_last_modified_date(),
            last_modified_by: self.audit.wire_last_modified_by(),
            source: self.source.clone(),
            path_options: self.path_options.clone(),
            tags: wire_tags(&self.tags),
            resource_arn: Some(self.resource_arn.clone()),
        }
    }
}

impl RecipeRecord {
    /// The recipe `principal` created at `at`: one working version holding the
    /// definition the request carried, and no published version, since a create
    /// publishes nothing.
    pub fn created(
        name: String,
        resource_arn: String,
        tags: BTreeMap<String, String>,
        description: Option<String>,
        steps: Vec<RecipeStep>,
        at: SystemTime,
        principal: String,
    ) -> Self {
        Self {
            name,
            project_name: None,
            tags,
            resource_arn,
            working: RecipeVersionRecord {
                version: INITIAL_RECIPE_VERSION.to_string(),
                description,
                steps,
                published_by: None,
                published_date: None,
                audit: Audit::created(at, principal),
            },
            published: Vec::new(),
        }
    }

    /// Apply an `UpdateRecipe` request's definition onto this recipe's WORKING version
    /// and stamp that version's last-modified pair, so a later read of the draft serves
    /// what the update wrote.
    ///
    /// `UpdateRecipe` "modifies the definition of the LATEST_WORKING version" (model:
    /// `UpdateRecipe`), so the draft is the only version it touches: a version a publish
    /// froze keeps the definition it was frozen with, and the recipe's own members (the
    /// name, the ARN, the tags, the project it is attached to) carry no member on this
    /// request at all. It is a PUT carrying the whole definition (model: `UpdateRecipe`
    /// `PUT /recipes/{Name}`, both definition members optional), so each definition
    /// member is REPLACED by what the request carries: an omitted `Steps` leaves the
    /// draft performing no step, exactly as an empty list does, and an omitted
    /// `Description` leaves it with none — the same rule [`DatasetRecord::apply_update`]
    /// applies to the other PUT update of a stored definition.
    pub fn apply_working_update(
        &mut self,
        description: Option<String>,
        steps: Vec<RecipeStep>,
        at: SystemTime,
        principal: String,
    ) {
        self.working.description = description;
        self.working.steps = steps;
        self.working.audit.modified(at, principal);
    }

    /// The version a request's `RecipeVersion` names, or `None` when this recipe holds
    /// no such version.
    ///
    /// The identifier is either a numeric `X.Y` version or one of the two aliases the
    /// model documents, and omitting it asks for "the latest published version"
    /// (model: `DescribeRecipeRequest$RecipeVersion`) — so an absent identifier
    /// resolves exactly what [`LATEST_PUBLISHED`] does and nothing else: a recipe no
    /// publish has frozen a version of holds no latest-published version, and the
    /// working draft is NOT a fallback for it. `LATEST_WORKING`, or the working
    /// version's own number, is how that draft is addressed.
    pub fn resolve_version(&self, requested: Option<&str>) -> Option<&RecipeVersionRecord> {
        match requested_recipe_version(requested) {
            LATEST_PUBLISHED => self.published.last(),
            LATEST_WORKING => Some(&self.working),
            version if version == self.working.version => Some(&self.working),
            version => self.published.iter().find(|held| held.version == version),
        }
    }

    /// Whether this recipe holds a version BESIDE its working one, which is what keeps
    /// that working version from being deleted (model: `BatchDeleteRecipeVersion` — "The
    /// LATEST_WORKING version will only be deleted if the recipe has no other versions").
    ///
    /// The question is asked of the versions the recipe holds when the request ARRIVES —
    /// "if you try to delete LATEST_WORKING while other versions exist" — so a request
    /// that names the working version together with every other one still runs into it.
    pub fn holds_other_versions(&self) -> bool {
        !self.published.is_empty()
    }

    /// The number the NEXT publish of this recipe freezes a version at: the major
    /// after the highest one this recipe holds, at minor 0.
    ///
    /// A version number is `X.Y`, major and minor, and `"0.0"` is not a valid one
    /// (model: `Recipe$RecipeVersion`); a recipe is created at major 0
    /// ([`INITIAL_RECIPE_VERSION`]), so the first publish of any recipe is `1.0` and
    /// each later one takes the next whole number.
    pub fn next_published_version(&self) -> String {
        let highest = self
            .published
            .iter()
            .chain(::std::iter::once(&self.working))
            .filter_map(|held| version_major(&held.version))
            .max()
            .unwrap_or(0);
        format!("{}.0", highest.saturating_add(1))
    }

    /// Renumber the working draft onto the highest major version this recipe holds
    /// published.
    ///
    /// The draft's number is a FUNCTION of the publish history rather than a number of
    /// its own: the working version is the first MINOR revision on top of the latest
    /// published major version, which is why a recipe no publish has frozen a version
    /// of sits at `0.1` ([`INITIAL_RECIPE_VERSION`]) — major 0, nothing published — and
    /// a recipe whose newest published version is major X carries its draft at `X.1`.
    /// So an operation that changes which versions this recipe holds published ends by
    /// calling this. [`RecipeRecord::publish`] is the one that does today; the rule binds
    /// any later handler that removes a published version too, since the draft's number
    /// is read off the history rather than stored independently of it. The draft keeps
    /// its definition and its audit across the renumber: it is the same draft, named for
    /// the major it now revises.
    fn renumber_working(&mut self) {
        // The HIGHEST published major, which is the same derivation
        // [`RecipeRecord::next_published_version`] reads the history with, so the two
        // never disagree about which major the recipe has reached.
        let published_major = self
            .published
            .iter()
            .filter_map(|held| version_major(&held.version))
            .max()
            .unwrap_or(0);
        self.working.version = format!("{published_major}.1");
    }

    /// Freeze this recipe's working version as a new published version, and answer the
    /// number it was frozen at.
    ///
    /// Publishing is ADDITIVE to the version history: it copies the definition the
    /// working version holds into a version of its own, numbered by
    /// [`RecipeRecord::next_published_version`] and appended to
    /// [`RecipeRecord::published`] (oldest first, so it becomes `LATEST_PUBLISHED`),
    /// while the working version carries on with the same definition as the draft
    /// development continues on. `description` is the description the publish carried
    /// for this version; a publish that carried none freezes the definition's own
    /// description with it.
    ///
    /// The draft is RENUMBERED onto the major just frozen
    /// ([`RecipeRecord::renumber_working`]): the draft revises the latest published
    /// major version, so publishing `X.0` leaves it at `X.1`. Its definition and its
    /// audit are untouched — a publish freezes a copy, it does not edit the draft.
    pub fn publish(
        &mut self,
        description: Option<String>,
        at: SystemTime,
        principal: String,
    ) -> String {
        let frozen = RecipeVersionRecord {
            version: self.next_published_version(),
            description: description.or_else(|| self.working.description.clone()),
            steps: self.working.steps.clone(),
            published_by: Some(principal.clone()),
            published_date: Some(at),
            // The frozen version is minted by this publish, so its create audit is
            // the publish itself rather than the draft's.
            audit: Audit::created(at, principal),
        };
        let version = frozen.version.clone();
        self.published.push(frozen);
        self.renumber_working();
        version
    }

    /// Remove the published versions `versions` names from this recipe's history, and
    /// renumber the working draft onto the major it now revises.
    ///
    /// A delete of a published version changes which versions the recipe holds, so the
    /// draft's number — a function of that history, not a number of its own
    /// ([`RecipeRecord::renumber_working`]) — is owed the same recomputation a publish
    /// owes it: dropping the newest published major leaves the draft revising the
    /// highest major that is left, and dropping every published version leaves it back
    /// at major 0. The draft keeps its definition and its audit: a delete of another
    /// version does not edit it.
    pub fn delete_published_versions(&mut self, versions: &[String]) {
        self.published
            .retain(|held| !versions.contains(&held.version));
        self.renumber_working();
    }

    /// The version `held` of this recipe as the modelled `Recipe` a listing carries.
    ///
    /// A `Recipe` item is one VERSION of a recipe (model: `Recipe$RecipeVersion`), so
    /// the definition members and the audit are the version's own — the same values
    /// `DescribeRecipe` answers for that version — while the members that belong to the
    /// recipe as a whole (its name, its ARN, its tags and the project it is attached to)
    /// are the record's, whichever of its versions is reported.
    pub fn version_to_wire(&self, held: &RecipeVersionRecord) -> Recipe {
        Recipe {
            created_by: Some(held.audit.created_by.clone()),
            create_date: Some(wire_date(held.audit.create_date)),
            // Absent until a mutating operation has modified this version.
            last_modified_by: held.audit.wire_last_modified_by(),
            last_modified_date: held.audit.wire_last_modified_date(),
            project_name: self.project_name.clone(),
            // Absent on a version no publish has frozen.
            published_by: held.published_by.clone(),
            published_date: held.published_date.map(wire_date),
            description: held.description.clone(),
            name: self.name.clone(),
            resource_arn: Some(self.resource_arn.clone()),
            steps: Some(held.steps.clone()),
            tags: wire_tags(&self.tags),
            recipe_version: Some(held.version.clone()),
        }
    }

    /// One page of the versions this recipe holds PUBLISHED, oldest first, from
    /// `resume_after` onwards.
    ///
    /// The published history is kept oldest first and numbered by increasing major
    /// version ([`RecipeRecord::publish`]), so the page walks it in version order and
    /// the returned token names the version this page ENDED on — the same contract
    /// [`page_map`] gives a name-keyed store, so a resumed call carries on at the next
    /// version rather than restarting. A token naming a version a later delete removed
    /// still resumes at the next version held, because the walk compares version
    /// numbers rather than positions.
    pub fn page_published_versions(
        &self,
        resume_after: Option<&str>,
        page_size: usize,
    ) -> Page<Recipe> {
        let after = resume_after.map(version_order);
        let mut remaining = self
            .published
            .iter()
            .filter(|held| after.is_none_or(|after| version_order(&held.version) > after));

        let mut items = Vec::new();
        let mut last_version: Option<String> = None;
        for held in remaining.by_ref().take(page_size) {
            items.push(self.version_to_wire(held));
            last_version = Some(held.version.clone());
        }

        Page {
            items,
            next_token: match (remaining.next().is_some(), last_version) {
                (true, Some(version)) => Some(encode_page_token(&version)),
                _ => None,
            },
        }
    }
}

/// A version identifier as the pair that ORDERS it against another: the major and
/// minor halves of a numeric `X.Y` version (model: `Recipe$RecipeVersion`).
///
/// An identifier that is not numeric — an alias, or a half too long to be a version
/// number — orders after every numeric one, so it never silently sorts among them.
fn version_order(version: &str) -> (u64, u64) {
    match version.split_once('.') {
        Some((major, minor)) => (
            major.parse().unwrap_or(u64::MAX),
            minor.parse().unwrap_or(u64::MAX),
        ),
        None => (u64::MAX, u64::MAX),
    }
}

/// The major half of a numeric `X.Y` version, or `None` when the identifier is not
/// numeric (an alias, say).
fn version_major(version: &str) -> Option<u64> {
    let (major, minor) = version.split_once('.')?;
    minor.parse::<u64>().ok()?;
    major.parse().ok()
}

impl ScheduleRecord {
    /// Apply an `UpdateSchedule` request's definition onto this record and stamp the
    /// audit's last-modified pair, so a later read serves what the update wrote.
    ///
    /// `UpdateSchedule` is a PUT that carries the schedule's whole definition (model:
    /// `UpdateSchedule` `PUT /schedules/{Name}` with `CronExpression` required and
    /// `JobNames` optional), so each definition member is REPLACED by what the request
    /// carries: an omitted `JobNames` leaves the schedule naming no job, exactly as an
    /// empty list does, rather than keeping the jobs it named before. `Name`, the ARN,
    /// the tags and the create pair are not this operation's to change — it carries no
    /// member for any of them.
    pub fn apply_update(
        &mut self,
        cron_expression: String,
        job_names: Vec<String>,
        at: SystemTime,
        principal: String,
    ) {
        self.cron_expression = cron_expression;
        self.job_names = job_names;
        self.audit.modified(at, principal);
    }
}

impl JobRunRecord {
    /// Materialise the transitions the clock owes this run by `now`.
    ///
    /// The terminal transition takes the state the run's EXECUTION is owed
    /// ([`RunOutcome`], decided when the run was admitted): a run whose recipe applies to
    /// its data ends `SUCCEEDED` reporting no `ErrorMessage`, and one whose recipe could
    /// not be applied ends `FAILED` reporting the diagnosis of the step that stopped it.
    ///
    /// `ExecutionTime` is the whole seconds the run consumed (model:
    /// "The amount of time, in seconds, during which the job run consumed
    /// resources"), measured from `StartedOn` to the instant the run completed.
    ///
    /// The terminal state is the one the run's WORK reached
    /// ([`JobRunRecord::terminal_outcome`]) — the clock decides WHEN it is reported, not
    /// WHICH it is. A run whose work never reported an outcome reaches `FAILED` carrying
    /// [`UNPERFORMED_RUN_MESSAGE`]: nothing performed it, so there is no success to
    /// report for it.
    pub fn advance_to(&mut self, now: SystemTime) {
        if self.state == "STARTING" && now >= self.started_on + RUN_RUNNING_DUE_AFTER {
            self.state = "RUNNING".to_string();
        }
        if self.state == "RUNNING" && now >= self.started_on + RUN_TERMINAL_DUE_AFTER {
            let completed_on = self.started_on + RUN_TERMINAL_DUE_AFTER;
            let outcome = self
                .terminal_outcome
                .clone()
                .unwrap_or_else(|| RunOutcome::failed(UNPERFORMED_RUN_MESSAGE.to_string()));
            self.state = outcome.state;
            self.error_message = outcome.error_message;
            self.completed_on = Some(completed_on);
            self.execution_time = completed_on
                .duration_since(self.started_on)
                .unwrap_or_default()
                .as_secs()
                .min(i32::MAX as u64) as i32;
        }
    }

    /// This record as the modelled `JobRun` that `ListJobRuns` returns.
    pub fn to_wire(&self) -> JobRun {
        JobRun {
            attempt: self.attempt,
            completed_on: self.completed_on.map(wire_date),
            dataset_name: self.dataset_name.clone(),
            error_message: self.error_message.clone(),
            execution_time: self.execution_time,
            job_name: Some(self.job_name.clone()),
            run_id: Some(self.run_id.clone()),
            state: Some(self.state.clone()),
            log_subscription: self.log_subscription.clone(),
            log_group_name: self.log_group_name.clone(),
            outputs: wire_list(&self.outputs),
            data_catalog_outputs: wire_list(&self.data_catalog_outputs),
            database_outputs: wire_list(&self.database_outputs),
            recipe_reference: self.recipe_reference.clone(),
            started_by: Some(self.started_by.clone()),
            started_on: Some(wire_date(self.started_on)),
            job_sample: None,
            validation_configurations: None,
        }
    }
}

/// An instant as the modelled `Date` timestamp.
pub fn wire_date(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// A stored tag map as the modelled `TagMap`: absent rather than empty, because
/// `TagMap` is `len 1..200` and an untagged resource carries no map at all.
pub fn wire_tags(tags: &BTreeMap<String, String>) -> Option<HashMap<String, String>> {
    if tags.is_empty() {
        None
    } else {
        Some(tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
    }
}

/// A stored list as a modelled `len 1..` list member: absent rather than empty.
pub fn wire_list<T: Clone>(items: &[T]) -> Option<Vec<T>> {
    if items.is_empty() {
        None
    } else {
        Some(items.to_vec())
    }
}

/// The modelled `TagMap` as stored tags.
pub fn stored_tags(tags: Option<&HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.map(|tags| tags.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default()
}

/// The region this request is scoped to: the one its SigV4 credential scope names,
/// else [`DEFAULT_REGION`].
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The IAM user name of the identity this emulator's long-term credentials speak for.
///
/// The emulator publishes exactly ONE long-term identity, and STS is what defines it:
/// `GetCallerIdentity` answers `arn:aws:iam::<account>:user/emulator` for every request
/// whose access key id names no session (`AMBIENT_USER_NAME` in
/// `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/src/state.rs`).
const AMBIENT_USER_NAME: &str = "emulator";

/// The principal that made this request, as the ARN DataBrew reports in
/// `CreatedBy`/`LastModifiedBy`/`StartedBy` (model: `DescribeRulesetResponse$CreatedBy`
/// — "The Amazon Resource Name (ARN) of the user who created the ...").
///
/// Server-derived from the emulator's own identity contract, never from the request:
/// it is the ARN of the identity a request signed with this emulator's credentials
/// speaks for, which is the one [`AMBIENT_USER_NAME`] names. An access key id is a
/// credential id, not a user name, so it is never rendered into an IAM ARN — a thing
/// no AWS rule produces. The value is therefore the same on every call and across
/// restarts, exactly as the IAM identity behind one set of credentials is.
pub fn request_principal() -> String {
    ::engine::emulator_arn("iam", "", &format!("user/{AMBIENT_USER_NAME}"))
}

/// The ARN of a DataBrew resource of `kind` (`dataset`, `job`, `recipe`, ...).
pub fn resource_arn(region: &str, kind: &str, name: &str) -> String {
    ::engine::emulator_arn("databrew", region, &format!("{kind}/{name}"))
}

/// The CloudWatch log group a job's runs log to, when the job subscribes to logs.
pub fn log_group_name(job_name: &str) -> String {
    format!("/aws-glue-databrew/jobs-{job_name}")
}

/// The `Source` a stored `Input` implies: which of the three input definitions it
/// carries (model: `Source` is one of `S3`, `DATA-CATALOG`, `DATABASE`). `None`
/// when the input carries none of them, which is not a dataset DataBrew can read.
pub fn source_of(input: &Input) -> Option<&'static str> {
    if input.s3_input_definition.is_some() {
        Some("S3")
    } else if input.data_catalog_input_definition.is_some() {
        Some("DATA-CATALOG")
    } else if input.database_input_definition.is_some() {
        Some("DATABASE")
    } else {
        None
    }
}

/// Refuse a name outside the character set DataBrew documents for it: "Valid
/// characters are alphanumeric (A-Z, a-z, 0-9), hyphen (-), period (.), and space"
/// (model: `CreateDatasetRequest$Name`, `CreateRecipeJobRequest$Name`). A name of
/// nothing but spaces names no resource, so it is refused too.
///
/// `Err` carries the message of the `ValidationException` to answer with; the
/// modelled length bounds are already enforced before a handler runs.
pub fn validate_resource_name(member: &str, value: &str) -> Result<(), String> {
    if value.trim().is_empty() {
        return Err(format!("{member} must not be blank"));
    }
    match value
        .chars()
        .find(|c| !(c.is_ascii_alphanumeric() || *c == '-' || *c == '.' || *c == ' '))
    {
        Some(bad) => Err(format!(
            "{member} contains the invalid character '{bad}': valid characters are \
             alphanumeric (A-Z, a-z, 0-9), hyphen (-), period (.), and space"
        )),
        None => Ok(()),
    }
}

/// Refuse a `RecipeVersion` that is not an identifier DataBrew documents.
///
/// The documented forms are a numeric `X.Y` version — major and minor, both required,
/// each at most 6 digits, neither negative, and not `0.0` — or the `LATEST_WORKING`
/// and `LATEST_PUBLISHED` aliases (model: `Recipe$RecipeVersion`). The modelled length
/// bounds (1..16) are already enforced before a handler runs.
///
/// `Err` carries the message of the `ValidationException` to answer with.
pub fn validate_recipe_version(member: &str, value: &str) -> Result<(), String> {
    if value == LATEST_WORKING || value == LATEST_PUBLISHED {
        return Ok(());
    }
    let malformed = || {
        Err(format!(
            "{member} must be a numeric version X.Y, with X and Y of at most \
             {RECIPE_VERSION_MAX_DIGITS} digits and not 0.0, or {LATEST_WORKING} or \
             {LATEST_PUBLISHED}"
        ))
    };
    let Some((major, minor)) = value.split_once('.') else {
        return malformed();
    };
    for half in [major, minor] {
        if half.is_empty()
            || half.len() > RECIPE_VERSION_MAX_DIGITS
            || !half.bytes().all(|byte| byte.is_ascii_digit())
        {
            return malformed();
        }
    }
    // "0.0" isn't a valid version, whichever way it is written.
    if major.bytes().chain(minor.bytes()).all(|byte| byte == b'0') {
        return malformed();
    }
    Ok(())
}

/// One field of a DataBrew cron expression: the values and wildcards it accepts.
///
/// The six fields, their ranges and their wildcards are the ones the Glue DataBrew
/// Developer Guide's "Cron expressions" page documents — the page the model's
/// `CreateScheduleRequest$CronExpression` doc links to
/// (<https://docs.aws.amazon.com/databrew/latest/dg/jobs.cron.html>):
/// `cron(minutes hours day-of-month month day-of-week year)`, with minutes 0-59,
/// hours 0-23, day-of-month 1-31, month 1-12 or JAN-DEC, day-of-week 1-7 or
/// SUN-SAT, and year 1970-2199.
struct CronField {
    /// The lowest numeric value the field accepts.
    min: u32,
    /// The highest numeric value the field accepts.
    max: u32,
    /// The aliases the field accepts, the one for [`CronField::min`] first.
    names: &'static [&'static str],
    /// Whether `?` ("no specific value") is a value of this field.
    question: bool,
    /// Whether `L` ("last") is a value of this field.
    last: bool,
    /// Whether `W` ("nearest weekday") is a value of this field.
    weekday: bool,
    /// Whether `#` ("nth day of the month") is a value of this field.
    nth: bool,
}

/// The month aliases the guide lists for the month field, January first.
const MONTH_NAMES: &[&str] = &[
    "JAN", "FEB", "MAR", "APR", "MAY", "JUN", "JUL", "AUG", "SEP", "OCT", "NOV", "DEC",
];

/// The day aliases the guide lists for the day-of-week field. Sunday is the field's
/// lowest value (1), which is where AWS cron differs from standard cron's 0.
const DAY_OF_WEEK_NAMES: &[&str] = &["SUN", "MON", "TUE", "WED", "THU", "FRI", "SAT"];

/// The six fields of a DataBrew cron expression, in the order they are written.
const CRON_FIELDS: [CronField; 6] = [
    // minutes
    CronField {
        min: 0,
        max: 59,
        names: &[],
        question: false,
        last: false,
        weekday: false,
        nth: false,
    },
    // hours
    CronField {
        min: 0,
        max: 23,
        names: &[],
        question: false,
        last: false,
        weekday: false,
        nth: false,
    },
    // day-of-month
    CronField {
        min: 1,
        max: 31,
        names: &[],
        question: true,
        last: true,
        weekday: true,
        nth: false,
    },
    // month
    CronField {
        min: 1,
        max: 12,
        names: MONTH_NAMES,
        question: false,
        last: false,
        weekday: false,
        nth: false,
    },
    // day-of-week
    CronField {
        min: 1,
        max: 7,
        names: DAY_OF_WEEK_NAMES,
        question: true,
        last: true,
        weekday: false,
        nth: true,
    },
    // year
    CronField {
        min: 1970,
        max: 2199,
        names: &[],
        question: false,
        last: false,
        weekday: false,
        nth: false,
    },
];

/// The message DataBrew answers a `ValidationException` with when a schedule's
/// `CronExpression` is not a cron expression it can read.
///
/// One constant sentence for the whole class: the service names no part of the
/// request in it and reports the expected FORM rather than which field is wrong, so
/// it is the message for every way an expression can fail that form.
pub const INVALID_CRON_EXPRESSION_MESSAGE: &str =
    "Invalid schedule cron expression. Expected format: Cron(CRON_EXPRESSION)";

/// Whether `CronExpression` is a cron expression DataBrew accepts.
///
/// A schedule runs its jobs on the dates and times its cron expression names, so the
/// expression must be one: `cron(` + the six documented fields + `)`, each field a
/// comma-separated list of values, ranges, steps and the wildcards that field accepts,
/// with `?` in exactly one of day-of-month and day-of-week. The modelled `len 1..512`
/// bound is already enforced before a handler runs; the FORM is not, so anything else
/// — a bare field list with no `cron(..)` around it, a rate expression, free text, a
/// value outside a field's published range — is not one, and the operation refuses it
/// with [`INVALID_CRON_EXPRESSION_MESSAGE`].
pub fn cron_expression_is_valid(value: &str) -> bool {
    let Some(body) = value
        .strip_prefix("cron(")
        .and_then(|body| body.strip_suffix(')'))
    else {
        return false;
    };

    let fields: Vec<&str> = body.split_whitespace().collect();
    if fields.len() != CRON_FIELDS.len() {
        return false;
    }

    if !fields
        .iter()
        .zip(CRON_FIELDS.iter())
        .all(|(field, spec)| cron_field_is_valid(spec, field))
    {
        return false;
    }

    // The guide's limit: "You can't specify the Day-of-month and Day-of-week fields in
    // the same cron expression. If you specify a value (or a *) in one of the fields,
    // you must use a ? (question mark) in the other." So exactly one of the two is `?`.
    let day_of_month_unspecified = fields[2] == "?";
    let day_of_week_unspecified = fields[4] == "?";
    day_of_month_unspecified != day_of_week_unspecified
}

/// Whether one field of a cron expression is one `spec` accepts: the single value `?`
/// where the field has it, else a comma-separated list of terms.
fn cron_field_is_valid(spec: &CronField, field: &str) -> bool {
    if field == "?" {
        return spec.question;
    }
    field
        .split(',')
        .all(|term| cron_term_is_valid(spec, term))
}

/// Whether one comma-separated term of a field is one `spec` accepts: `*`, a value, a
/// `low-high` range, any of those with a `/step` increment, or one of the field's
/// wildcards (`L`, `LW`, `<day>W`, `<day>L`, `<day>#<nth>`).
fn cron_term_is_valid(spec: &CronField, term: &str) -> bool {
    let (base, step) = match term.split_once('/') {
        Some((base, step)) => (base, Some(step)),
        None => (term, None),
    };
    // A step is the increment between values, so it is a positive integer.
    if let Some(step) = step {
        if cron_number(step).is_none_or(|step| step < 1) {
            return false;
        }
    }
    if base == "*" {
        return true;
    }
    if let Some((low, high)) = base.split_once('-') {
        return match (cron_value(spec, low), cron_value(spec, high)) {
            (Some(low), Some(high)) => low <= high,
            _ => false,
        };
    }
    // `L` is the last day of the month or week and `LW` the last weekday of the month.
    if spec.last && (base == "L" || (spec.weekday && base == "LW")) {
        return true;
    }
    if spec.weekday {
        if let Some(day) = base.strip_suffix('W') {
            return cron_value(spec, day).is_some();
        }
    }
    if spec.last {
        if let Some(day) = base.strip_suffix('L') {
            return cron_value(spec, day).is_some();
        }
    }
    if spec.nth {
        if let Some((day, nth)) = base.split_once('#') {
            // `#` takes the 1st to 5th such day of the month.
            return cron_value(spec, day).is_some()
                && cron_number(nth).is_some_and(|nth| (1..=5).contains(&nth));
        }
    }
    cron_value(spec, base).is_some()
}

/// One value of `spec`'s field: a number in its range, or one of its aliases.
fn cron_value(spec: &CronField, token: &str) -> Option<u32> {
    let alias = token.to_ascii_uppercase();
    if let Some(index) = spec.names.iter().position(|name| *name == alias) {
        return Some(spec.min + index as u32);
    }
    cron_number(token).filter(|value| (spec.min..=spec.max).contains(value))
}

/// A cron token as a plain decimal number: digits only, so a sign or a stray
/// character is not a cron value.
fn cron_number(token: &str) -> Option<u32> {
    if token.is_empty() || !token.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}

/// One page of a listing, and the token that resumes after it.
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// Page `map` (a name-keyed store) from `resume_after` onwards, WITHOUT
/// materialising it: the walk is a `BTreeMap` range that starts after the token's
/// key and stops at the end of the page, so neither the work nor the allocation is
/// sized by the store. `to_item` converts only the entries on this page.
///
/// The returned token is opaque and names where this page ENDED, so the next call
/// resumes at the following entry rather than restarting.
pub fn page_map<V, T>(
    map: &BTreeMap<String, V>,
    resume_after: Option<&str>,
    page_size: usize,
    to_item: impl Fn(&V) -> T,
) -> Page<T> {
    let lower = match resume_after {
        Some(key) => Bound::Excluded(key.to_string()),
        None => Bound::Unbounded,
    };

    let mut entries = map.range((lower, Bound::Unbounded));
    let mut items = Vec::new();
    let mut last_key: Option<String> = None;
    for (key, value) in entries.by_ref().take(page_size) {
        items.push(to_item(value));
        last_key = Some(key.clone());
    }

    Page {
        items,
        next_token: match (entries.next().is_some(), last_key) {
            (true, Some(key)) => Some(encode_page_token(&key)),
            _ => None,
        },
    }
}

/// The page size to serve a request that asked for `max_results`, clamped
/// server-side to the modelled 1..100 so a caller cannot ask for an unbounded page.
pub fn page_size(max_results: Option<i32>) -> usize {
    max_results.unwrap_or(MAX_PAGE_SIZE).clamp(1, MAX_PAGE_SIZE) as usize
}

/// The key a request's `nextToken` resumes after, or the `ValidationException`
/// message to answer with for a token this service did not mint.
pub fn page_resume_after(next_token: Option<&str>) -> Result<Option<String>, String> {
    match next_token {
        Some(token) => decode_page_token(token).map(Some),
        None => Ok(None),
    }
}

/// The opaque pagination token naming the last key of a page.
fn encode_page_token(key: &str) -> String {
    hex::encode(key.as_bytes())
}

/// The key an opaque pagination token names, or the `ValidationException` message
/// for a token this service did not mint.
fn decode_page_token(token: &str) -> Result<String, String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .ok_or_else(|| "The nextToken is not a token this service issued".to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDatabrewAwsgluedatabrewState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDatabrewAwsgluedatabrewState>)
}
