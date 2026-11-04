// SPDX-License-Identifier: CC0-1.0

//! The RUNS a workflow holds: one record per execution, and the store they live in.
//!
//! A custom workflow step hands its target resource an `ExecutionId`, a `WorkflowId`
//! and a `Token`, and the step stays pending until `SendWorkflowStepState` reports an
//! outcome for that token. An execution exists only while a workflow runs over an
//! uploaded file, so this is the state that callback is resolved against — and the
//! record `ListExecutions` and `DescribeExecution` report from.
//!
//! The records are written by the one thing that starts a run — a file arriving on a
//! server whose `WorkflowDetails` attaches the workflow
//! ([`super::connector_delivery`]) — and the store they live in is BOUNDED here, per
//! workflow and in total, with the runs a workflow no longer needs released rather than
//! held for the life of the process.

use ::std::collections::BTreeMap;

use super::{
    ComAmazonawsTransferTransferserviceState, CUSTOM_STEP_STATUS_FAILURE,
    CUSTOM_STEP_STATUS_SUCCESS, EXECUTION_STATUS_COMPLETED, EXECUTION_STATUS_EXCEPTION,
    EXECUTION_STATUS_IN_PROGRESS, MAX_EXECUTIONS_PER_WORKFLOW, MAX_STORED_EXECUTIONS,
};
use crate::ops::types::FileLocation as ExecutionFileLocation;
use crate::ops::types::ServiceMetadata as ExecutionServiceMetadata;

/// One workflow execution's custom-step callbacks, keyed by `(WorkflowId,
/// ExecutionId)`.
#[derive(Debug, Clone, Default)]
pub struct WorkflowExecution {
    pub workflow_id: String,
    pub execution_id: String,
    /// "the file location when the execution begins" (`ListedExecution`
    /// `InitialFileLocation`) — the uploaded file the run started on. `None` for a run
    /// recorded with no file location, which reports none rather than an empty
    /// structure.
    pub initial_file_location: Option<ExecutionFileLocation>,
    /// "the session details that are associated with a workflow": the server, user and
    /// session the upload that started this run arrived on. `None` when the record
    /// carries none.
    pub service_metadata: Option<ExecutionServiceMetadata>,
    /// The outcome reported for each pending custom step, by the `Token` that step was
    /// handed: `None` until its callback arrives, then the `CustomStepStatus` it
    /// carried. A token this map does not hold addresses no custom step of this
    /// execution.
    pub step_states: BTreeMap<String, Option<String>>,
}

impl WorkflowExecution {
    /// The `ExecutionStatus` a reader reports for this run, DERIVED from the custom-step
    /// callbacks recorded on it rather than stored — like `UserCount`, so it cannot go
    /// stale when a callback arrives.
    ///
    /// The custom-step callbacks are the WHOLE of what a run waits on: a custom step is
    /// the one step type that hands its target a `Token` and stays pending until
    /// `SendWorkflowStepState` reports an outcome for it, while every other
    /// `WorkflowStepType` (`COPY`, `TAG`, `DELETE`, `DECRYPT`) is applied by the service
    /// itself with nothing to wait for. So:
    ///
    /// * a step that reported `FAILURE` is the "exception encountered" the
    ///   `ExecutionStatus` value `EXCEPTION` names;
    /// * a run whose every custom step has reported `SUCCESS` has nothing left to wait
    ///   for, so it is `COMPLETED` — vacuously so for a run with no custom step at all,
    ///   which is a workflow of non-custom steps alone;
    /// * otherwise a step is still pending and the run is `IN_PROGRESS`.
    ///
    /// `HANDLING_EXCEPTION` is never reported: it is the window a run spends executing
    /// its `OnExceptionSteps`, and this emulator executes no step, so no record it holds
    /// is inside that window.
    pub fn reported_status(&self) -> &'static str {
        let reported_failure = self
            .step_states
            .values()
            .any(|outcome| outcome.as_deref() == Some(CUSTOM_STEP_STATUS_FAILURE));
        let every_step_succeeded = self
            .step_states
            .values()
            .all(|outcome| outcome.as_deref() == Some(CUSTOM_STEP_STATUS_SUCCESS));
        if reported_failure {
            EXECUTION_STATUS_EXCEPTION
        } else if every_step_succeeded {
            EXECUTION_STATUS_COMPLETED
        } else {
            EXECUTION_STATUS_IN_PROGRESS
        }
    }
}

impl ComAmazonawsTransferTransferserviceState {
    /// The executions stored for `workflow_id`, in `ListExecutions` paging order.
    ///
    /// A RANGE scan over that workflow's contiguous run of the execution map, on the
    /// same rule as [`Self::users_of`].
    pub fn executions_of<'a>(
        &'a self,
        workflow_id: &'a str,
    ) -> impl Iterator<Item = &'a WorkflowExecution> + 'a {
        use ::std::ops::Bound;
        let lower = (workflow_id.to_string(), String::new());
        self.workflow_executions
            .range((Bound::Included(lower), Bound::Unbounded))
            .take_while(move |((wid, _), _)| wid == workflow_id)
            .map(|(_, execution)| execution)
    }

    /// Whether `workflow_id` can take another execution, RECLAIMING finished runs when
    /// a bound has been reached.
    ///
    /// The execution store is bounded twice — per workflow
    /// ([`MAX_EXECUTIONS_PER_WORKFLOW`]) and in total ([`MAX_STORED_EXECUTIONS`]) — so
    /// a caller that keeps uploading cannot grow it without end. At a bound, the runs
    /// whose custom-step callbacks have FINISHED are released first: `DescribeExecution`
    /// "only returns details for in-progress workflows", so a finished run is a record
    /// nothing addresses any more, and releasing it is what lets the next upload run.
    /// `false` when even that leaves no room, and then no execution is started: a
    /// record the store cannot hold is not written, exactly as every other `MAX_*` bound
    /// in [`crate::state`] refuses rather than growing.
    ///
    /// Called by the one writer that starts a run ([`super::connector_delivery`]), under
    /// the same lock the record is inserted through.
    pub fn take_execution_capacity(&mut self, workflow_id: &str) -> bool {
        if !self.has_execution_capacity(workflow_id) {
            self.workflow_executions
                .retain(|_, execution| execution.reported_status() == EXECUTION_STATUS_IN_PROGRESS);
        }
        self.has_execution_capacity(workflow_id)
    }

    /// Whether both execution bounds have room for one more run of `workflow_id`.
    fn has_execution_capacity(&self, workflow_id: &str) -> bool {
        self.workflow_executions.len() < MAX_STORED_EXECUTIONS
            && self.executions_of(workflow_id).count() < MAX_EXECUTIONS_PER_WORKFLOW
    }

    /// Release every execution of `workflow_id`.
    ///
    /// A deleted workflow has no runs: an execution is addressed by its
    /// `(WorkflowId, ExecutionId)` pair, so once the workflow is gone nothing can
    /// resolve one of its records, and `DeleteWorkflow` releases them with it — the
    /// same rule `DeleteConnector` follows for a connector's accepted transfers.
    pub fn release_executions_of(&mut self, workflow_id: &str) {
        self.workflow_executions
            .retain(|(wid, _), _| wid != workflow_id);
    }
}
