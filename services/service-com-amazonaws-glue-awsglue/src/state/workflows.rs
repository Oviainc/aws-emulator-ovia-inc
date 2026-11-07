// SPDX-License-Identifier: CC0-1.0

//! The stored workflow record and the blueprint run that laid it out.

use super::*;

/// A stored workflow: the container for the dependent jobs and crawlers
/// `CreateWorkflow` registers, and the resource an EVENT trigger attaches to.
///
/// Every member of the modelled `Workflow` shape a create can set is held here, plus the
/// create's `Tags`, which that shape does not carry. The server-set members are the
/// instants the record was written at and the runs started of it; the member only a
/// BLUEPRINT produces (`BlueprintDetails`) belongs to the operations that produce it, so a
/// workflow a create wrote carries none.
#[derive(Debug, Clone)]
pub struct WorkflowEntry {
    /// The workflow's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::workflows`].
    pub name: String,
    /// "A description of the workflow", stored exactly as sent, and absent for a create
    /// that named none.
    pub description: Option<String>,
    /// "A collection of properties to be used as part of each execution of the workflow."
    /// A create that named none stores NO properties rather than an empty map, so a read
    /// reports the absence the create expressed.
    pub default_run_properties: Option<HashMap<String, String>>,
    /// The ceiling on this workflow's concurrent runs. "If you leave this parameter
    /// blank, there is no limit to the number of concurrent workflow runs", so an absent
    /// member is stored absent rather than as a number.
    pub max_concurrent_runs: Option<i32>,
    /// Server-set, written once at create so every later read reports the same instant.
    pub created_on: SystemTime,
    /// Server-set; moves with every write to this workflow.
    pub last_modified_on: SystemTime,
    /// The create's `Tags`, which are not part of the `Workflow` shape.
    ///
    /// Kept so the create does not discard what the client sent, but NOT yet reachable
    /// through the tagging operations, for the reason [`CustomEntityTypeEntry::tags`]
    /// gives: [`TaggedResourceKind`] carries no workflow variant yet, and resolving one
    /// belongs to the tagging operations' own issues.
    pub tags: HashMap<String, String>,
    /// The runs `StartWorkflowRun` started of this workflow, oldest first and bounded by
    /// [`MAX_RUNS_PER_WORKFLOW`].
    ///
    /// A run lives on its workflow because every operation that addresses one names the
    /// workflow too: `GetWorkflowRun` takes a `Name` beside the `RunId`, exactly as
    /// `GetJobRun` takes a `JobName` beside its run id.
    pub runs: Vec<WorkflowRunEntry>,
    /// The blueprint run that created this workflow, and `None` for a workflow a create
    /// registered directly.
    ///
    /// "The details of the blueprint that this particular workflow is created from"
    /// (`Workflow$BlueprintDetails`) is what a read reports it as, so a workflow a blueprint
    /// run laid out names the run it came from and one `CreateWorkflow` wrote names none.
    pub blueprint_run: Option<WorkflowBlueprintRun>,
}

/// Store `entry` in `workflows` under its own name, the key every workflow read addresses it
/// by.
///
/// The ONE place this namespace's key form lives, so a write that reaches the map through a
/// split borrow of the store ([`ComAmazonawsGlueAwsglueState::start_blueprint_run`]) keys it
/// exactly as [`ComAmazonawsGlueAwsglueState::insert_workflow`] does.
pub(super) fn insert_workflow_into(workflows: &mut BTreeMap<String, WorkflowEntry>, entry: WorkflowEntry) {
    workflows.insert(entry.name.clone(), entry);
}

/// The blueprint run a workflow was created by: "The details of a blueprint"
/// (`BlueprintDetails`), which is the blueprint and the run of it that laid the workflow out.
#[derive(Debug, Clone)]
pub struct WorkflowBlueprintRun {
    /// `BlueprintDetails$BlueprintName`: "The name of the blueprint."
    pub blueprint_name: String,
    /// `BlueprintDetails$RunId`: "The run ID for this blueprint."
    pub run_id: String,
}

impl WorkflowEntry {
    /// Build a stored workflow from a `CreateWorkflow` request plus the one thing the
    /// SERVICE fills: the instant the record was written at, which is both its create
    /// and its last-modified instant until a write moves the latter.
    pub fn from_create_input(input: &CreateWorkflowInput, now: SystemTime) -> Self {
        Self {
            name: input.name.clone(),
            description: input.description.clone(),
            default_run_properties: input.default_run_properties.clone(),
            max_concurrent_runs: input.max_concurrent_runs,
            created_on: now,
            last_modified_on: now,
            tags: input.tags.clone().unwrap_or_default(),
            // A workflow a create wrote has not been run.
            runs: Vec::new(),
            // "The details of the blueprint that this particular workflow is created from":
            // a workflow `CreateWorkflow` registered was created from none.
            blueprint_run: None,
        }
    }

    /// Build the workflow a blueprint run lays out: the workflow that run produces, carrying
    /// the blueprint run it was created from.
    ///
    /// The members a `CreateWorkflow` sets — a description, default run properties, a
    /// concurrency ceiling, tags — are defined by the blueprint's published archive, which
    /// nothing here reads, so the workflow is registered with none of them rather than with
    /// values invented for it; the same goes for the jobs, crawlers and triggers that archive
    /// lays out INSIDE the workflow, so none of those is created either.
    pub fn from_blueprint_run(
        name: &str,
        blueprint_name: &str,
        run_id: &str,
        now: SystemTime,
    ) -> Self {
        Self {
            name: name.to_string(),
            description: None,
            default_run_properties: None,
            max_concurrent_runs: None,
            created_on: now,
            last_modified_on: now,
            tags: HashMap::new(),
            // A workflow the run has just laid out has not been run.
            runs: Vec::new(),
            blueprint_run: Some(WorkflowBlueprintRun {
                blueprint_name: blueprint_name.to_string(),
                run_id: run_id.to_string(),
            }),
        }
    }

    /// Apply an `UpdateWorkflow` request to this stored workflow, member by member, as
    /// served at `now`.
    ///
    /// Every member the request CARRIES replaces the stored one and every member it OMITS
    /// is preserved: the request is a patch over the definition the create registered, the
    /// same way an `UpdateCrawler` ([`CrawlerEntry::apply_update`]) or an
    /// `UpdateMLTransform` ([`MlTransformEntry::apply_update`]) is, and unlike `UpdateJob`,
    /// whose submitted definition is documented to overwrite the stored one completely.
    /// `Name` ADDRESSES the workflow rather than changing it (`UpdateWorkflowRequest` has no
    /// rename member), and the request has no member for the create instant, the tags or the
    /// runs already started of the workflow, so none of those is this request's to change.
    ///
    /// A member whose value is a WHOLE collection is replaced whole rather than merged key
    /// by key: `DefaultRunProperties` is "a collection of properties to be used as part of
    /// each execution of the workflow", so a request naming that collection sets it, which is
    /// what makes merging the business of `PutWorkflowRunProperties` — the operation that
    /// writes individual properties, and onto one RUN rather than onto the workflow.
    ///
    /// The new defaults apply to the runs STARTED AFTER this write: a run already begun
    /// holds the properties it began with ([`WorkflowRunEntry::run_properties`]).
    /// `LastModifiedOn` becomes `now`, since it "moves with every write to this workflow".
    pub fn apply_update(&mut self, input: &UpdateWorkflowInput, now: SystemTime) {
        if let Some(description) = input.description.as_ref() {
            self.description = Some(description.clone());
        }
        if let Some(default_run_properties) = input.default_run_properties.as_ref() {
            self.default_run_properties = Some(default_run_properties.clone());
        }
        if let Some(max_concurrent_runs) = input.max_concurrent_runs {
            self.max_concurrent_runs = Some(max_concurrent_runs);
        }
        self.last_modified_on = now;
    }

    /// The maximum number of this workflow's runs that may be in flight at once, or `None`
    /// when the workflow names no ceiling: "If you leave this parameter blank, there is no
    /// limit to the number of concurrent workflow runs"
    /// (`CreateWorkflowRequest$MaxConcurrentRuns`).
    ///
    /// A named ceiling is held at one ([`DEFAULT_MAX_CONCURRENT_RUNS`]), so a stored zero
    /// cannot make the workflow unrunnable — the same floor a job's concurrency gets.
    pub fn concurrent_run_ceiling(&self) -> Option<i32> {
        self.max_concurrent_runs
            .map(|ceiling| ceiling.max(DEFAULT_MAX_CONCURRENT_RUNS))
    }

    /// Give every run of this workflow the state the CLOCK already owed it at `now`.
    ///
    /// This is the one write a reporting operation performs, exactly as
    /// [`JobEntry::settle_due_runs`] is for a job run: a run's next status was due at
    /// [`WorkflowRunEntry::settles_at`] whether or not anybody called, so materialising it
    /// on read reports the status the clock had already reached.
    pub fn settle_due_runs(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            run.settle_if_due(now);
        }
    }

    /// The stored run with this id, or `None` when this workflow has no such run.
    pub fn run(&self, run_id: &str) -> Option<&WorkflowRunEntry> {
        self.runs.iter().find(|run| run.id == run_id)
    }

    /// [`Self::run`] mutably: where every write to one of this workflow's runs lands.
    pub fn run_mut(&mut self, run_id: &str) -> Option<&mut WorkflowRunEntry> {
        self.runs.iter_mut().find(|run| run.id == run_id)
    }

    /// How many of this workflow's runs have not finished.
    ///
    /// Call [`Self::settle_due_runs`] first, or a run the clock has already finished still
    /// counts as in flight.
    pub fn runs_in_flight(&self) -> usize {
        self.runs.iter().filter(|run| !run.is_terminal()).count()
    }

    /// Begin a run of this workflow at `now` and answer the id minted for it, or the
    /// concurrency ceiling the run would have exceeded.
    ///
    /// The ONE place a run of a workflow is recorded, whatever began it: a caller's
    /// `StartWorkflowRun`, or a trigger of this workflow that fired. Everything the clock
    /// already owed this workflow's runs is materialised first, so a run the clock has
    /// already finished does not count as one in flight, and the ceiling the workflow named
    /// is then what decides whether another run may begin.
    ///
    /// `requested_properties` are the run properties the caller asked this run to carry,
    /// and `None` for a run nobody passed any for — a trigger firing passes none, so such a
    /// run carries the workflow's own defaults. `total_actions` is how many actions the
    /// entry point that began the run initiates.
    pub fn begin_run(
        &mut self,
        requested_properties: Option<&HashMap<String, String>>,
        now: SystemTime,
        total_actions: i32,
    ) -> Result<String, i32> {
        self.settle_due_runs(now);
        // "You can use this parameter to prevent unwanted multiple updates to data … If you
        // leave this parameter blank, there is no limit to the number of concurrent workflow
        // runs": past a ceiling the workflow named, no further run begins.
        if let Some(ceiling) = self.concurrent_run_ceiling() {
            if self.runs_in_flight() >= usize::try_from(ceiling).unwrap_or(usize::MAX) {
                return Err(ceiling);
            }
        }
        let run_id = new_workflow_run_id();
        let run = WorkflowRunEntry::begin(
            self,
            requested_properties,
            run_id.clone(),
            now,
            total_actions,
        );
        self.record_run(run);
        Ok(run_id)
    }

    /// Append a run, keeping at most [`MAX_RUNS_PER_WORKFLOW`] of them.
    pub fn record_run(&mut self, run: WorkflowRunEntry) {
        self.runs.push(run);
        if self.runs.len() > MAX_RUNS_PER_WORKFLOW {
            let overflow = self.runs.len() - MAX_RUNS_PER_WORKFLOW;
            self.runs.drain(..overflow);
        }
    }

    /// Rebuild the modelled `Workflow` shape from what is stored, so a listing reports
    /// the same workflow a single read does.
    ///
    /// `graph` is the graph the reader asked for with its own `IncludeGraph` ("Specifies
    /// whether to include a graph when returning the workflow resource metadata") —
    /// [`ComAmazonawsGlueAwsglueState::workflow_graph`] for this workflow — and `None` when
    /// it asked for none, because an unasked-for graph is reported as no graph at all.
    /// `last_run` is "the information about the last execution of the workflow" projected
    /// by the STORE, which holds the runs that execution's actions began
    /// ([`ComAmazonawsGlueAwsglueState::to_workflow`]).
    pub fn to_workflow(
        &self,
        graph: Option<WorkflowGraph>,
        last_run: Option<WorkflowRun>,
    ) -> Workflow {
        Workflow {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            default_run_properties: self.default_run_properties.clone(),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            // "The information about the last execution of the workflow": the most
            // recently started run this workflow holds, and nothing at all for a workflow
            // no run has been started of. The nested run carries no graph of its own —
            // `IncludeGraph` asks for the WORKFLOW's graph, which is reported beside it.
            last_run,
            graph,
            max_concurrent_runs: self.max_concurrent_runs,
            // "The details of the blueprint that this particular workflow is created
            // from": the blueprint run that laid this workflow out, and nothing at all for
            // a workflow a create registered directly.
            blueprint_details: self.blueprint_run.as_ref().map(|origin| BlueprintDetails {
                blueprint_name: Some(origin.blueprint_name.clone()),
                run_id: Some(origin.run_id.clone()),
            }),
        }
    }
}
