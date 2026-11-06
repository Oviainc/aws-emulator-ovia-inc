// SPDX-License-Identifier: CC0-1.0

//! The multi-Region setup tasks `CreateResourceExplorerSetup` starts and
//! `GetResourceExplorerSetup` reports on.
//!
//! A setup task is a COMPOSITE: for each Region it configures it owns two steps,
//! the Region's index and the Region's view, and the view step depends on the
//! index step — a view is queried through its Region's index, so it cannot exist
//! until that index is usable. Each step therefore carries the instant the clock
//! owes it its terminal status, computed when the task is recorded so that it
//! does not depend on when anybody looks, and [`super::ComAmazonawsResourceexplorer2ResourceexplorerState::settle`]
//! materialises it.
//!
//! Nothing here holds a status that is not backed by the index and view the step
//! is about: a step's `Index` / `View` member is read from the stored resource at
//! report time, and a step whose resource disappeared under it fails.

use super::*;

/// The most setup tasks this store keeps a record of.
///
/// CHOSEN, not documented: `CreateResourceExplorerSetup` is caller-driven and a task
/// record outlives the call, so without a ceiling a caller could grow the task log
/// without end. When the ceiling is reached the OLDEST task is dropped, and its id
/// then resolves to nothing — the same `ResourceNotFoundException` as an id this
/// store never issued.
pub const MAX_SETUP_TASKS: usize = 100;

/// The `ErrorDetails$Code` of a step whose Region no longer holds the index the
/// step was configuring, worded as the matching service exception.
pub const SETUP_INDEX_GONE_CODE: &str = "ResourceNotFoundException";

/// The `ErrorDetails$Code` of a step blocked by a resource that already exists in
/// the Region in a state the task cannot build on.
pub const SETUP_CONFLICT_CODE: &str = "ConflictException";

/// The `ErrorDetails$Code` of a step that would exceed a Resource Explorer quota.
pub const SETUP_QUOTA_CODE: &str = "ServiceQuotaExceededException";

/// One step of a setup task, as the `OperationStatus` enum names it on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupStepStatus {
    /// The clock still owes this step its terminal status.
    InProgress,
    /// The step configured the resource it is about.
    Succeeded,
    /// The step could not be carried out; `ErrorDetails` says why.
    Failed,
    /// The step had nothing to do: the Region already held the resource.
    Skipped,
}

impl SetupStepStatus {
    /// The `OperationStatus` value this status is called on the wire.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::InProgress => "IN_PROGRESS",
            Self::Succeeded => "SUCCEEDED",
            Self::Failed => "FAILED",
            Self::Skipped => "SKIPPED",
        }
    }

    /// Whether this status is the step's last: the clock owes it nothing more.
    pub fn is_terminal(self) -> bool {
        !matches!(self, Self::InProgress)
    }
}

/// Why one step of a setup task failed, as `ErrorDetails` reports it.
#[derive(Debug, Clone)]
pub struct SetupStepError {
    pub code: String,
    pub message: String,
}

/// One step of a setup task: one Region's index, or one Region's view.
#[derive(Debug, Clone)]
pub struct SetupStep {
    pub status: SetupStepStatus,
    /// The resource this step is about, once the task has one to name.
    pub arn: Option<String>,
    pub error: Option<SetupStepError>,
    /// When the clock owes this step its terminal status, while it is still in
    /// progress. Never slept on: the transition is materialised by whichever call
    /// next looks at the task.
    pub due_at: Option<SystemTime>,
}

impl SetupStep {
    /// A step the clock owes a terminal status at `due_at`, already naming the
    /// resource it is configuring when the task created one up front.
    pub fn in_progress(arn: Option<String>, due_at: SystemTime) -> Self {
        Self {
            status: SetupStepStatus::InProgress,
            arn,
            error: None,
            due_at: Some(due_at),
        }
    }

    /// A step with nothing to do, because `arn` already existed in the Region.
    pub fn skipped(arn: String) -> Self {
        Self {
            status: SetupStepStatus::Skipped,
            arn: Some(arn),
            error: None,
            due_at: None,
        }
    }

    /// A step that could not be carried out.
    pub fn failed(code: &str, message: String) -> Self {
        Self {
            status: SetupStepStatus::Failed,
            arn: None,
            error: Some(SetupStepError {
                code: code.to_string(),
                message,
            }),
            due_at: None,
        }
    }

    /// Whether this step is owed its terminal status at `now`.
    fn due(&self, now: SystemTime) -> bool {
        self.status == SetupStepStatus::InProgress && self.due_at.is_some_and(|at| at <= now)
    }

    /// The modelled `ErrorDetails` for this step, when it carries one.
    fn error_details(&self) -> Option<crate::ops::types::ErrorDetails> {
        self.error
            .as_ref()
            .map(|error| crate::ops::types::ErrorDetails {
                code: Some(error.code.clone()),
                message: Some(error.message.clone()),
            })
    }
}

/// One Region's share of a setup task.
#[derive(Debug, Clone)]
pub struct SetupRegionTask {
    pub region: String,
    pub index: SetupStep,
    pub view: SetupStep,
}

impl SetupRegionTask {
    /// Whether the clock owes this Region nothing more.
    pub fn is_settled(&self) -> bool {
        self.index.status.is_terminal() && self.view.status.is_terminal()
    }
}

/// One `CreateResourceExplorerSetup` call's work, as `GetResourceExplorerSetup`
/// reports it.
#[derive(Debug, Clone)]
pub struct SetupTask {
    /// The `TaskId` the creating call answered with.
    pub task_id: String,
    /// The name of the view the task creates in every Region it configures.
    pub view_name: String,
    /// The Regions the task configures, ordered by Region so that paging over
    /// them is stable.
    pub regions: Vec<SetupRegionTask>,
    pub created_at: SystemTime,
}

impl ComAmazonawsResourceexplorer2ResourceexplorerState {
    /// Record `task`, dropping the oldest task first when the store is already
    /// holding [`MAX_SETUP_TASKS`] of them.
    pub fn insert_setup_task(&mut self, task: SetupTask) {
        while self.setup_tasks.len() >= MAX_SETUP_TASKS {
            let Some(oldest) = self
                .setup_tasks
                .values()
                .min_by_key(|held| held.created_at)
                .map(|held| held.task_id.clone())
            else {
                break;
            };
            self.setup_tasks.remove(&oldest);
        }
        self.setup_tasks.insert(task.task_id.clone(), task);
    }

    /// Materialise every setup-task step the clock already owes at `now`.
    ///
    /// Called from [`Self::settle`] AFTER the index lifecycle it depends on, so a
    /// task's view step sees the index its own Region's step was waiting for.
    pub(super) fn settle_setup_tasks(&mut self, now: SystemTime) {
        let task_ids: Vec<String> = self
            .setup_tasks
            .iter()
            .filter(|(_, task)| task.regions.iter().any(|entry| !entry.is_settled()))
            .map(|(task_id, _)| task_id.clone())
            .collect();
        for task_id in task_ids {
            let Some(task) = self.setup_tasks.get(&task_id) else {
                continue;
            };
            let view_name = task.view_name.clone();
            // Worked on a copy, so the Region's index and views stay reachable
            // through `self` while the steps are being advanced.
            let mut regions = task.regions.clone();
            for entry in &mut regions {
                self.settle_index_step(entry, now);
                self.settle_view_step(entry, &view_name, now);
            }
            if let Some(task) = self.setup_tasks.get_mut(&task_id) {
                task.regions = regions;
            }
        }
    }

    /// Advance one Region's index step: it succeeds once the Region's index is
    /// `ACTIVE` and the step is due, and fails if that index went away under it.
    fn settle_index_step(&mut self, entry: &mut SetupRegionTask, now: SystemTime) {
        if !entry.index.due(now) {
            return;
        }
        match self.indexes.get(&entry.region).map(|index| index.state) {
            Some(IndexLifecycle::Active) => {
                let arn = self.indexes.get(&entry.region).map(|index| index.arn.clone());
                entry.index.status = SetupStepStatus::Succeeded;
                entry.index.arn = arn;
                entry.index.due_at = None;
            }
            // The index is taking longer to come up, or to switch type, than the
            // step's own due instant; it stays in progress until the index is usable.
            Some(IndexLifecycle::Creating) | Some(IndexLifecycle::Updating) => {}
            // The Region's index was deleted, or is being deleted, since the task
            // started: there is nothing left for this step to have configured.
            _ => {
                let error = SetupStepError {
                    code: SETUP_INDEX_GONE_CODE.to_string(),
                    message: format!(
                        "Resource Explorer is no longer turned on in Region {}.",
                        entry.region
                    ),
                };
                entry.index.status = SetupStepStatus::Failed;
                entry.index.error = Some(error.clone());
                entry.index.due_at = None;
                if !entry.view.status.is_terminal() {
                    entry.view.status = SetupStepStatus::Failed;
                    entry.view.error = Some(error);
                    entry.view.due_at = None;
                }
            }
        }
    }

    /// Advance one Region's view step, which the index step gates: once the
    /// Region's index is usable and the step is due, the view the task was asked
    /// for is created there.
    fn settle_view_step(&mut self, entry: &mut SetupRegionTask, view_name: &str, now: SystemTime) {
        if !entry.view.due(now) || !entry.index.status.is_terminal() {
            return;
        }
        if entry.index.status == SetupStepStatus::Failed {
            entry.view.status = SetupStepStatus::Failed;
            entry.view.error = entry.index.error.clone();
            entry.view.due_at = None;
            return;
        }
        // The view is reached through the Region's index, so it waits for one that
        // is usable rather than being created beside a missing index.
        let index_usable = self
            .live_index(&entry.region)
            .is_some_and(|index| index.state == IndexLifecycle::Active);
        if !index_usable {
            return;
        }
        // "The name must be unique within its Amazon Web Services Region": a view
        // of that name already there is the view the setup asked for.
        if let Some(existing) = self.view_by_name(&entry.region, view_name) {
            entry.view = SetupStep::skipped(existing.view_arn.clone());
            return;
        }
        let held = self.views_in_region(&entry.region).len();
        if held >= VIEWS_PER_REGION_QUOTA {
            entry.view = SetupStep::failed(
                SETUP_QUOTA_CODE,
                format!(
                    "Region {} already holds the maximum number of views ({held}).",
                    entry.region
                ),
            );
            return;
        }
        let arn = self
            .insert_view(
                NewView {
                    view_name: view_name.to_string(),
                    region: entry.region.clone(),
                    ..NewView::default()
                },
                now,
            )
            .view_arn;
        entry.view.status = SetupStepStatus::Succeeded;
        entry.view.arn = Some(arn);
        entry.view.due_at = None;
    }

    /// The modelled `RegionStatus` for one Region's share of a task.
    ///
    /// The ONE place a `RegionStatus` is built. Each step's resource is read from
    /// the store at report time, so a step never names an index or a view that is
    /// no longer there.
    pub fn region_status(&self, entry: &SetupRegionTask) -> crate::ops::types::RegionStatus {
        let index = entry
            .index
            .arn
            .as_deref()
            .and_then(|arn| self.indexes.values().find(|index| index.arn == arn))
            .map(IndexEntry::to_wire);
        let view = entry
            .view
            .arn
            .as_deref()
            .and_then(|arn| self.views.get(arn))
            .map(ViewEntry::to_wire);
        crate::ops::types::RegionStatus {
            region: Some(entry.region.clone()),
            index: Some(crate::ops::types::IndexStatus {
                status: Some(entry.index.status.as_wire().to_string()),
                index,
                error_details: entry.index.error_details(),
            }),
            view: Some(crate::ops::types::ViewStatus {
                status: Some(entry.view.status.as_wire().to_string()),
                view,
                error_details: entry.view.error_details(),
            }),
        }
    }
}
