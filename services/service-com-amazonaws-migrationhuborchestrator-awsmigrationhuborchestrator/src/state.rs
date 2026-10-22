// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-migrationhuborchestrator-awsmigrationhuborchestrator service.
//!
//! This module owns the migration-workflow store every workflow handler in
//! `src/ops/` reads and writes, plus the few helpers those handlers share (the
//! request's region, the workflow ARN shape, the opaque list cursor, and the
//! clock-owed status transition). Keeping them here means the six workflow
//! operations agree on one record rather than each holding its own copy.

use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{MigrationWorkflowSummary, StepInput};

/// The `MigrationWorkflowStatusEnum` values this slice moves a workflow through.
///
/// Each one is a member of `MIGRATION_WORKFLOW_STATUS_ENUM_VALUES`
/// (`src/ops/types/shared_types.rs`, generated from the Smithy model), which is the
/// only place the wire-legal set is defined.
pub const STATUS_CREATING: &str = "CREATING";
/// See [`STATUS_CREATING`].
pub const STATUS_NOT_STARTED: &str = "NOT_STARTED";
/// See [`STATUS_CREATING`].
pub const STATUS_STARTING: &str = "STARTING";
/// See [`STATUS_CREATING`].
pub const STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`STATUS_CREATING`].
pub const STATUS_PAUSING: &str = "PAUSING";
/// See [`STATUS_CREATING`].
pub const STATUS_PAUSED: &str = "PAUSED";
/// See [`STATUS_CREATING`].
pub const STATUS_DELETING: &str = "DELETING";

/// How long a workflow sits in its transient status before the clock settles it
/// (`CREATING` -> `NOT_STARTED`, `STARTING` -> `IN_PROGRESS`, `PAUSING` -> `PAUSED`).
///
/// CHOSEN, not documented by AWS: the real service takes however long the work takes.
/// The span is deliberately non-zero so a caller that reads straight back still
/// observes the transient status the create/start/stop call reported.
pub const STATUS_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// The most workflows this store holds at once.
///
/// An EMULATOR RESOURCE GUARD, not an AWS-published quota: the store is in memory
/// and grows one record per CreateWorkflow, so the count is bounded and a create
/// past the bound is refused rather than letting a caller grow the process without
/// limit. Per-record size is already bounded by the modelled input constraints,
/// which the framework validates before a handler runs.
pub const MAX_LIVE_WORKFLOWS: usize = 1_000;

/// The page size `ListWorkflows` uses when the caller supplies no `maxResults`.
///
/// `maxResults` is modelled `range 0..100` with `default 0`, so an absent member
/// arrives as 0; answering that with the modelled maximum keeps one request's
/// response and work bounded instead of serialising the whole store.
pub const DEFAULT_PAGE_SIZE: usize = 100;

/// The region an ARN carries when the request is unsigned, so no SigV4 credential
/// scope names one.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN namespace / SigV4 signing name of this service, per `OPERATIONS.md`.
const ARN_SERVICE: &str = "migrationhub-orchestrator";

/// One stored migration workflow: everything CreateWorkflow persisted plus whatever
/// later calls changed. Every workflow response is read back out of this record.
#[derive(Debug, Clone)]
pub struct Workflow {
    /// Creation position, strictly increasing. It is what a `ListWorkflows` cursor
    /// names, so a page resumes correctly even when the workflow the previous page
    /// ended on has since been deleted.
    pub seq: u64,
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub template_id: String,
    pub ads_application_configuration_id: Option<String>,
    /// The Application Discovery Service application's NAME. This slice has no ADS
    /// integration, so nothing resolves a configuration id to a name and the member
    /// stays absent; `ListWorkflows`' `adsApplicationConfigurationName` filter
    /// compares against it rather than inventing one.
    pub ads_application_configuration_name: Option<String>,
    pub status: String,
    pub status_message: Option<String>,
    pub workflow_inputs: HashMap<String, StepInput>,
    pub step_targets: Vec<String>,
    pub tags: HashMap<String, String>,
    pub creation_time: ::aws_smithy_types::DateTime,
    pub last_start_time: Option<::aws_smithy_types::DateTime>,
    pub last_stop_time: Option<::aws_smithy_types::DateTime>,
    pub last_modified_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
    pub total_steps: Option<i32>,
    pub completed_steps: Option<i32>,
    pub workflow_bucket: Option<String>,
    /// The status the clock owes this workflow, and the instant it comes due.
    /// `None` once the workflow has settled.
    pending_status: Option<(String, Instant)>,
}

impl Workflow {
    /// Put the workflow into `transient` now and owe `settled` to the clock.
    pub fn begin_transition(&mut self, transient: &str, settled: &str) {
        self.status = transient.to_string();
        self.pending_status = Some((settled.to_string(), Instant::now() + STATUS_SETTLE_AFTER));
    }

    /// Materialise a status transition the clock has already made due.
    ///
    /// This is the one write a reporting operation (GetWorkflow, ListWorkflows) may
    /// make: the transition was owed at this instant whether or not anybody read the
    /// workflow, so applying it changes nothing a later caller would otherwise see.
    pub fn settle(&mut self) {
        if let Some((settled, due)) = self.pending_status.take() {
            if Instant::now() >= due {
                self.status = settled;
            } else {
                self.pending_status = Some((settled, due));
            }
        }
    }

    /// Whether the workflow is RUNNING, the state DeleteWorkflow's modelled
    /// documentation says must be paused before the workflow can be deleted
    /// ("You must pause a running workflow in Migration Hub Orchestrator console
    /// to delete it.").
    pub fn is_running(&self) -> bool {
        self.status == STATUS_STARTING || self.status == STATUS_IN_PROGRESS
    }

    /// This workflow as a `ListWorkflows` summary entry.
    pub fn summary(&self) -> MigrationWorkflowSummary {
        MigrationWorkflowSummary {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            template_id: Some(self.template_id.clone()),
            ads_application_configuration_name: self.ads_application_configuration_name.clone(),
            status: Some(self.status.clone()),
            creation_time: Some(self.creation_time),
            end_time: self.end_time,
            status_message: self.status_message.clone(),
            completed_steps: self.completed_steps,
            total_steps: self.total_steps,
        }
    }
}

/// What a caller asked CreateWorkflow to persist, after the framework validated it.
#[derive(Debug, Clone, Default)]
pub struct NewWorkflow {
    pub name: String,
    pub description: Option<String>,
    pub template_id: String,
    pub ads_application_configuration_id: Option<String>,
    pub workflow_inputs: HashMap<String, StepInput>,
    pub step_targets: Vec<String>,
    pub tags: HashMap<String, String>,
}

/// Per-service state for com-amazonaws-migrationhuborchestrator-awsmigrationhuborchestrator. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsMigrationhuborchestratorAwsmigrationhuborchestratorState {
    /// Every live workflow, keyed by its generated id.
    pub workflows: HashMap<String, Workflow>,
    /// Creation order, so `ListWorkflows` pages deterministically. Ids are removed
    /// here when the workflow is deleted, keeping it in step with `workflows`.
    pub workflow_order: Vec<String>,
    /// Monotonic counter behind each workflow's `seq` and generated id.
    workflow_seq: u64,
}

impl ComAmazonawsMigrationhuborchestratorAwsmigrationhuborchestratorState {
    /// Persist a new workflow and return the stored record.
    ///
    /// `None` when the store already holds [`MAX_LIVE_WORKFLOWS`] workflows; the
    /// caller refuses the request rather than growing the store further.
    pub fn create_workflow(&mut self, region: &str, spec: NewWorkflow) -> Option<Workflow> {
        if self.workflows.len() >= MAX_LIVE_WORKFLOWS {
            return None;
        }
        self.workflow_seq = self.workflow_seq.wrapping_add(1);
        let seq = self.workflow_seq;
        let id = generated_workflow_id(seq);
        let arn = workflow_arn(region, &id);
        let mut workflow = Workflow {
            seq,
            id,
            arn,
            name: spec.name,
            description: spec.description,
            template_id: spec.template_id,
            ads_application_configuration_id: spec.ads_application_configuration_id,
            ads_application_configuration_name: None,
            status: STATUS_CREATING.to_string(),
            status_message: None,
            workflow_inputs: spec.workflow_inputs,
            step_targets: spec.step_targets,
            tags: spec.tags,
            creation_time: now(),
            last_start_time: None,
            last_stop_time: None,
            last_modified_time: None,
            end_time: None,
            // No workflow steps exist until the step operations are implemented, so
            // the step counters have no value to report yet.
            total_steps: None,
            completed_steps: None,
            workflow_bucket: None,
            pending_status: None,
        };
        workflow.begin_transition(STATUS_CREATING, STATUS_NOT_STARTED);
        self.workflow_order.push(workflow.id.clone());
        self.workflows.insert(workflow.id.clone(), workflow.clone());
        Some(workflow)
    }

    /// Remove a workflow from both the map and the creation order.
    pub fn remove_workflow(&mut self, id: &str) -> Option<Workflow> {
        let removed = self.workflows.remove(id)?;
        self.workflow_order.retain(|known| known != id);
        Some(removed)
    }

    /// The workflow under `id`, with any clock-owed status transition applied.
    pub fn settled_workflow(&mut self, id: &str) -> Option<&Workflow> {
        let workflow = self.workflows.get_mut(id)?;
        workflow.settle();
        Some(workflow)
    }

    /// Apply every clock-owed status transition that has come due.
    pub fn settle_all(&mut self) {
        for workflow in self.workflows.values_mut() {
            workflow.settle();
        }
    }

    /// Every live workflow in creation order. Borrowed, so a reader that only needs
    /// a page does not copy the whole store.
    pub fn workflows_in_creation_order(&self) -> impl Iterator<Item = &Workflow> {
        self.workflow_order
            .iter()
            .filter_map(|id| self.workflows.get(id))
    }
}

/// A workflow id matching `MigrationWorkflowId`'s `^[a-zA-Z0-9-]+$` / len 1..100
/// (see `OPERATIONS.md`). The creation counter is what keeps it unique; the clock
/// reading only keeps ids from reading as a dense sequence, and neither part is a
/// secret — nothing in this service authorises on an id.
fn generated_workflow_id(seq: u64) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos() as u64)
        & 0xffff_ffff_ffff;
    format!("wf-{seq:08x}{nanos:012x}")
}

/// The current server clock as a modelled timestamp.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
}

/// The region the caller signed for, or [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map_or_else(|| DEFAULT_REGION.to_string(), |scope| scope.region)
}

/// `arn:aws:migrationhub-orchestrator:<region>:<account>:workflow/<id>`.
pub fn workflow_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("workflow/{id}"))
}

/// The prefix a cursor carries, so a token this service did not mint is rejected
/// rather than read as a position.
const PAGE_TOKEN_PREFIX: &str = "mhow1:";

/// Encode a list cursor: the [`Workflow::seq`] of the LAST entry of the page just
/// returned, so the next page resumes after it whatever happens to that workflow
/// in between.
///
/// Opaque to the caller (hex of the encoded position) and non-whitespace, which is
/// what `nextToken`'s `.*\S.*` / len 0..2048 constraints require.
pub fn encode_page_token(last_seq: u64) -> String {
    ::hex::encode(format!("{PAGE_TOKEN_PREFIX}{last_seq}"))
}

/// The position a cursor from [`encode_page_token`] names, or `None` when the token
/// was not produced by this service.
pub fn decode_page_token(token: &str) -> Option<u64> {
    let decoded = String::from_utf8(::hex::decode(token).ok()?).ok()?;
    decoded.strip_prefix(PAGE_TOKEN_PREFIX)?.parse().ok()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsMigrationhuborchestratorAwsmigrationhuborchestratorState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsMigrationhuborchestratorAwsmigrationhuborchestratorState>)
}
