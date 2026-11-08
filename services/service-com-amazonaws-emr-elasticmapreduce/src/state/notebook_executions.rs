// SPDX-License-Identifier: CC0-1.0

//! The notebook-execution lifecycle, one concern of this service's state.
//!
//! `StartNotebookExecution` records a [`NotebookExecutionRecord`] here and the clock walks
//! it through the statuses `NotebookExecution$Status` names
//! ([`NOTEBOOK_EXECUTION_PROGRESSION`], [`NOTEBOOK_EXECUTION_STOP_PROGRESSION`]);
//! `DescribeNotebookExecution` reports that record and `ListNotebookExecutions` a
//! PROJECTION of it ([`NotebookExecutionRecord::to_summary`]). Its names are re-exported by
//! the state root, so every caller keeps reaching them through `crate::state`.

use super::*;

/// The ARN of a notebook execution:
/// `arn:aws:elasticmapreduce:<region>:<account>:notebook-execution/<id>`.
pub fn notebook_execution_arn(region: &str, execution_id: &str) -> String {
    ::engine::emulator_arn(
        "elasticmapreduce",
        region,
        &format!("notebook-execution/{execution_id}"),
    )
}

/// `ListNotebookExecutions` page size: "Returns a maximum of 50 notebook executions and
/// a marker to track the paging of a longer notebook execution list across multiple
/// `ListNotebookExecutions` calls." (`ListNotebookExecutions` doc, Smithy model.)
pub const LIST_NOTEBOOK_EXECUTIONS_PAGE_SIZE: usize = 50;

/// How far back `ListNotebookExecutions` looks when the request names no `From`: "The
/// beginning of time range filter for listing notebook executions. The default is the
/// timestamp of 30 days ago." (`ListNotebookExecutionsInput$From` doc, Smithy model.)
pub const NOTEBOOK_EXECUTION_DEFAULT_FROM: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The statuses a notebook execution passes through, in order, from the moment the cluster
/// has received the request until the execution has ended.
///
/// Straight off `NotebookExecution$Status`: "`START_PENDING` indicates that the cluster has
/// received the execution request but execution has not begun", "`STARTING` indicates that
/// the execution is starting on the cluster", "`RUNNING` indicates that the execution is
/// being processed by the cluster", "`FAILING` indicates that the execution is failing and
/// will not finish successfully" and "`FAILED` indicates that the execution failed". Each
/// move is owed [`NOTEBOOK_PHASE_SPAN`] after the one before it.
///
/// It ends in the FAILURE statuses because this service runs no notebook: it records the
/// execution faithfully — its engine, its notebook location, its parameters — and walks the
/// phases the cluster would take it through, but no cell of the notebook is executed, so
/// the execution reaches no result. `FINISHED` ("the execution has completed without
/// error") would be a success for work nothing performed, which is why
/// [`NOTEBOOK_NOT_EXECUTED_REASON`] stands in `LastStateChangeReason` instead and says so.
pub const NOTEBOOK_EXECUTION_PROGRESSION: &[&str] = &[
    notebook_execution_status::START_PENDING,
    notebook_execution_status::STARTING,
    notebook_execution_status::RUNNING,
    notebook_execution_status::FAILING,
    notebook_execution_status::FAILED,
];

/// `NotebookExecution$LastStateChangeReason` for an execution this service recorded and
/// walked the phases of without running its notebook — the reason that tells a caller the
/// failure is this emulator's own and not the notebook's.
pub const NOTEBOOK_NOT_EXECUTED_REASON: &str =
    "This emulator records notebook executions but runs no notebook, so the execution \
     ended without running.";

/// The statuses a notebook execution passes through once a caller has asked it to stop:
/// "`STOP_PENDING` indicates that the cluster has received a `StopNotebookExecution`
/// request and the stop is pending", "`STOPPING` indicates that the cluster is in the
/// process of stopping the execution" and "`STOPPED` indicates that the execution stopped
/// because of a `StopNotebookExecution` request" (`NotebookExecution$Status` doc).
pub const NOTEBOOK_EXECUTION_STOP_PROGRESSION: &[&str] = &[
    notebook_execution_status::STOP_PENDING,
    notebook_execution_status::STOPPING,
    notebook_execution_status::STOPPED,
];

/// The status that follows `status` in the progression it belongs to, or `None` for one
/// that ends its progression and for one no progression names.
///
/// The ONE reading of what a notebook execution's clock owes it next, so the start
/// progression and the stop progression cannot drift apart from the statuses above.
pub fn next_notebook_execution_status(status: &str) -> Option<&'static str> {
    for progression in [
        NOTEBOOK_EXECUTION_PROGRESSION,
        NOTEBOOK_EXECUTION_STOP_PROGRESSION,
    ] {
        if let Some(position) = progression.iter().position(|step| *step == status) {
            return progression.get(position + 1).copied();
        }
    }
    None
}

/// `NotebookExecution$LastStateChangeReason` for an execution a caller stopped.
pub const NOTEBOOK_STOPPED_REASON: &str = "Stopped by a StopNotebookExecution request";

/// The status a notebook execution's clock owes it, and when that comes due.
#[derive(Debug, Clone)]
pub struct PendingNotebookExecutionStatus {
    /// The instant the status below was owed, stored rather than slept for.
    pub due: SystemTime,
    pub status: String,
    /// The reason the move records, for a move that has one to give.
    pub reason: Option<String>,
}

/// A persisted notebook execution: the wire `NotebookExecution` that
/// `DescribeNotebookExecution` reports, plus the status the clock still owes it.
///
/// `ListNotebookExecutions` reports a PROJECTION of this record
/// ([`NotebookExecutionRecord::to_summary`]); no handler builds that summary from its
/// own input.
#[derive(Debug, Clone)]
pub struct NotebookExecutionRecord {
    pub execution: NotebookExecution,
    pub pending_status: Option<PendingNotebookExecutionStatus>,
}

impl NotebookExecutionRecord {
    /// The execution `StartNotebookExecution` records for `input`, under the id and ARN
    /// EMR minted for it.
    pub fn from_request(
        id: String,
        arn: String,
        input: StartNotebookExecutionInput,
        now: SystemTime,
    ) -> Self {
        Self {
            execution: NotebookExecution {
                notebook_execution_id: Some(id),
                editor_id: input.editor_id,
                execution_engine: Some(input.execution_engine),
                notebook_execution_name: input.notebook_execution_name,
                notebook_params: input.notebook_params,
                // "START_PENDING indicates that the cluster has received the execution
                // request but execution has not begun." (`NotebookExecution$Status`
                // doc.) That is exactly where a just-recorded execution stands.
                status: Some(notebook_execution_status::START_PENDING.to_string()),
                start_time: Some(timestamp(now)),
                end_time: None,
                arn: Some(arn),
                // "The location of the notebook execution's output file in Amazon S3"
                // (`NotebookExecution$OutputNotebookURI` doc): no cell of the notebook has
                // run yet, so there is no output file to point at.
                output_notebook_uri: None,
                last_state_change_reason: None,
                notebook_instance_security_group_id: input.notebook_instance_security_group_id,
                tags: input.tags,
                notebook_s3_location: input.notebook_s3_location.map(|location| {
                    NotebookS3LocationForOutput {
                        bucket: location.bucket,
                        key: location.key,
                    }
                }),
                output_notebook_s3_location: input.output_notebook_s3_location.map(|location| {
                    OutputNotebookS3LocationForOutput {
                        bucket: location.bucket,
                        key: location.key,
                    }
                }),
                output_notebook_format: input.output_notebook_format,
                environment_variables: input.environment_variables,
            },
            // The execution request has been received; the clock owes it the next status of
            // the progression ([`NOTEBOOK_EXECUTION_PROGRESSION`]) and then each one after
            // that, up to the terminal one. A progression that names nothing after
            // `START_PENDING` owes NOTHING — the execution then simply stays where it is,
            // rather than being defaulted into a status nothing took it to.
            pending_status: next_notebook_execution_status(
                notebook_execution_status::START_PENDING,
            )
            .map(|next| PendingNotebookExecutionStatus {
                due: now + NOTEBOOK_PHASE_SPAN,
                status: next.to_string(),
                reason: Self::phase_reason(next),
            }),
        }
    }

    /// Materialise every status the clock already made due, and nothing else.
    ///
    /// An execution moves one phase at a time, so a read that comes several phases late
    /// applies each of them in turn and reports the one the clock has actually reached —
    /// the answer does not depend on how often anybody looked.
    pub fn advance_to(&mut self, now: SystemTime) {
        while let Some(pending) = self.pending_status.clone() {
            if now < pending.due {
                return;
            }
            self.pending_status = None;
            self.execution.status = Some(pending.status.clone());
            if let Some(reason) = pending.reason {
                self.execution.last_state_change_reason = Some(reason);
            }
            // A status the execution never leaves is where it ended, and the instant it
            // ended is the one the transition was owed at — not the instant somebody
            // looked.
            if notebook_execution_status::TERMINAL.contains(&pending.status.as_str()) {
                self.execution.end_time = Some(timestamp(pending.due));
                return;
            }
            // Whatever the phase that just landed owes next, one span later.
            if let Some(next) = next_notebook_execution_status(&pending.status) {
                self.pending_status = Some(PendingNotebookExecutionStatus {
                    due: pending.due + NOTEBOOK_PHASE_SPAN,
                    status: next.to_string(),
                    reason: Self::phase_reason(next),
                });
            }
        }
    }

    /// The reason a move to `status` records, for a move that has one to give: the stop
    /// progression says the caller asked for the stop, the two failure phases say the
    /// notebook was never run ([`NOTEBOOK_NOT_EXECUTED_REASON`]), and the phases on the way
    /// have nothing to report.
    fn phase_reason(status: &str) -> Option<String> {
        if NOTEBOOK_EXECUTION_STOP_PROGRESSION.contains(&status) {
            return Some(NOTEBOOK_STOPPED_REASON.to_string());
        }
        if status == notebook_execution_status::FAILING
            || status == notebook_execution_status::FAILED
        {
            return Some(NOTEBOOK_NOT_EXECUTED_REASON.to_string());
        }
        None
    }

    /// Begin stopping this execution on the caller's request, answering whether there
    /// was anything to stop.
    ///
    /// "STOP_PENDING indicates that the cluster has received a `StopNotebookExecution`
    /// request and the stop is pending" (`NotebookExecution$Status` doc), so the stop is
    /// asynchronous: the execution goes `STOP_PENDING` now and the clock owes it the rest
    /// of the stop progression ([`NOTEBOOK_EXECUTION_STOP_PROGRESSION`]), which abandons
    /// whatever the run still owed. An execution already stopping was accepted once, so
    /// repeating the request does not push that due instant out.
    pub fn begin_stop(&mut self, now: SystemTime) -> bool {
        let status = self.execution.status.as_deref().unwrap_or_default();
        if notebook_execution_status::TERMINAL.contains(&status) {
            return false;
        }
        if NOTEBOOK_EXECUTION_STOP_PROGRESSION.contains(&status) {
            return true;
        }
        self.execution.status = Some(notebook_execution_status::STOP_PENDING.to_string());
        self.execution.last_state_change_reason = Some(NOTEBOOK_STOPPED_REASON.to_string());
        self.pending_status =
            next_notebook_execution_status(notebook_execution_status::STOP_PENDING).map(|next| {
                PendingNotebookExecutionStatus {
                    due: now + NOTEBOOK_PHASE_SPAN,
                    status: next.to_string(),
                    reason: Some(NOTEBOOK_STOPPED_REASON.to_string()),
                }
            });
        true
    }

    /// The wire `NotebookExecutionSummary` for this record.
    pub fn to_summary(&self) -> NotebookExecutionSummary {
        NotebookExecutionSummary {
            notebook_execution_id: self.execution.notebook_execution_id.clone(),
            editor_id: self.execution.editor_id.clone(),
            notebook_execution_name: self.execution.notebook_execution_name.clone(),
            status: self.execution.status.clone(),
            start_time: self.execution.start_time,
            end_time: self.execution.end_time,
            notebook_s3_location: self.execution.notebook_s3_location.clone(),
            execution_engine_id: self
                .execution
                .execution_engine
                .as_ref()
                .map(|engine| engine.id.clone()),
        }
    }

    /// The id of the execution engine this execution names, which
    /// `ListNotebookExecutions$ExecutionEngineId` filters on.
    pub fn execution_engine_id(&self) -> Option<&str> {
        self.execution
            .execution_engine
            .as_ref()
            .map(|engine| engine.id.as_str())
    }
}
