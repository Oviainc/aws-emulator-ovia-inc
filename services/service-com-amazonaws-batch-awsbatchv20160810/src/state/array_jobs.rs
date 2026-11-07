// SPDX-License-Identifier: CC0-1.0

//! ARRAY jobs: the children a submit spawns, and the parent they add up to.
//!
//! `SubmitJob$arrayProperties`: "If you specify array properties for a job, it becomes an
//! array job", and `ArrayProperties$size` is "the size of the array job". AWS Batch answers
//! such a submit with one job id — the PARENT — and spawns `size` children under it, each
//! carrying its own `ArrayPropertiesDetail$index` ("the job index within the array that's
//! associated with this job"), which is the index its container reads to pick the shard of
//! work it owns. A child's id is the parent's own id with its index after a colon, which is
//! the form `ListJobs$arrayJobId` ("lists all child jobs from within the specified array")
//! and every job-addressing call take.
//!
//! The parent runs no container: `JobDetail$createdAt` documents the division — for a parent
//! it is "when the job entered the SUBMITTED state ... specifically at the time SubmitJob was
//! called", while for "array child jobs, this is when the child job was spawned by its
//! parent". So the parent's own status is what its children add up to: it reaches a terminal
//! status when every child has, `SUCCEEDED` when they all succeeded and `FAILED` otherwise,
//! and `ArrayPropertiesDetail$statusSummary` ("a summary of the number of array job children
//! in each available job status") is counted from the children's records on every read rather
//! than kept beside them, so it cannot drift from the jobs it counts.

use super::*;

/// The id of the child of `parent_id` at `index`: the parent's id, a colon and the index.
///
/// The form every array-aware member of the API uses — `ListJobs$arrayJobId` lists the
/// children of a parent id, and a child is described, cancelled and terminated under this
/// id like any other job.
pub fn array_child_job_id(parent_id: &str, index: i32) -> String {
    format!("{parent_id}:{index}")
}

/// What one CHILD of an array job depends on, given the dependencies its parent's submit
/// named and the child's own `index`.
///
/// `SubmitJob$dependsOn` documents both array forms, and this is where they are applied:
///
/// * "You can specify a SEQUENTIAL type dependency without specifying a job ID for array
///   jobs so that each child array job completes sequentially, starting at index 0" — so
///   the child at `index` waits for the child at `index - 1` of its OWN array, and the
///   child at index 0 waits for nothing.
/// * "You can also specify an N_TO_N type dependency with a job ID for array jobs. In that
///   case, each index child of this job must wait for the corresponding index child of each
///   dependency" — so the entry is re-pointed at the dependency's child of the SAME index,
///   when the store holds one.
///
/// Every other entry is inherited as written: a dependency on a plain job holds the whole
/// array back, which is what a job id with no array form means.
pub fn array_child_dependencies(
    parent_dependencies: Option<&Vec<JobDependency>>,
    parent_id: &str,
    index: i32,
    holds: impl Fn(&str) -> bool,
) -> Option<Vec<JobDependency>> {
    let mapped: Vec<JobDependency> = parent_dependencies
        .into_iter()
        .flatten()
        .filter_map(|dependency| {
            let kind = dependency.r#type.as_deref();
            match (kind, dependency.job_id.as_deref()) {
                (Some(ARRAY_DEPENDENCY_N_TO_N), Some(depended_on)) => {
                    let matching = array_child_job_id(depended_on, index);
                    let job_id = if holds(&matching) {
                        matching
                    } else {
                        depended_on.to_string()
                    };
                    Some(JobDependency {
                        job_id: Some(job_id),
                        r#type: dependency.r#type.clone(),
                    })
                }
                (Some(ARRAY_DEPENDENCY_SEQUENTIAL), None) => (index > 0).then(|| JobDependency {
                    job_id: Some(array_child_job_id(parent_id, index - 1)),
                    r#type: dependency.r#type.clone(),
                }),
                (_, Some(_)) => Some(dependency.clone()),
                (_, None) => None,
            }
        })
        .collect();
    (!mapped.is_empty()).then_some(mapped)
}

/// `ArrayJobDependency$N_TO_N` — the dependency type that pairs children by index.
pub const ARRAY_DEPENDENCY_N_TO_N: &str = "N_TO_N";

/// `ArrayJobDependency$SEQUENTIAL` — the dependency type that orders an array's own
/// children.
pub const ARRAY_DEPENDENCY_SEQUENTIAL: &str = "SEQUENTIAL";

/// `ArrayProperties$size` must be at least this.
///
/// `SubmitJob$arrayProperties`: "The array size can be between 2 and 10,000", so a submit
/// outside that range names no array AWS Batch would create. The member targets a plain
/// `Integer` (the model declares no range), so the bound is applied by the handler.
pub const MIN_ARRAY_SIZE: i32 = 2;

/// `ArrayProperties$size` must be at most this ("between 2 and 10,000", as above).
pub const MAX_ARRAY_SIZE: i32 = 10_000;

/// The detail a submit whose `arrayProperties.size` is outside the documented range is
/// refused with: the documented bound, stated as the frontend validation that rejects it
/// states a refused argument.
pub fn array_size_refusal() -> String {
    format!("Array size must be between {MIN_ARRAY_SIZE} and {MAX_ARRAY_SIZE}.")
}

impl ComAmazonawsBatchAwsbatchv20160810State {
    /// The children this store holds for the array parent `parent_id`, in index order.
    ///
    /// Reached through the ids the parent's size names
    /// ([`array_child_job_id`]) rather than by scanning the store, so the cost is the
    /// array's own size and nothing else. Empty for every job that is not an array parent.
    pub fn array_children(&self, parent_id: &str) -> Vec<&Job> {
        let size = self
            .jobs
            .get(parent_id)
            .and_then(|parent| parent.array_size)
            .unwrap_or_default();
        (0..size)
            .filter_map(|index| self.jobs.get(&array_child_job_id(parent_id, index)))
            .collect()
    }

    /// `ArrayPropertiesDetail$statusSummary` for the array parent `parent_id`: how many of
    /// its children are in each `JobStatus`, and the latest instant one of them moved.
    ///
    /// Every modelled status is a key, including the ones no child is in, which is how AWS
    /// Batch reports the summary. The counts are read off the children's own records on
    /// every call, so no caller can see a summary that disagrees with the children it
    /// summarises. `None` for a job that is not an array parent.
    pub fn array_status_summary(&self, parent_id: &str) -> Option<(HashMap<String, i32>, i64)> {
        let parent = self.jobs.get(parent_id)?;
        parent.array_size?;
        let mut summary: HashMap<String, i32> = crate::ops::types::JOB_STATUS_VALUES
            .iter()
            .map(|status| ((*status).to_string(), 0))
            .collect();
        let mut last_updated = parent.status_since;
        for child in self.array_children(parent_id) {
            *summary.entry(child.status.clone()).or_default() += 1;
            last_updated = last_updated.max(child.status_since);
        }
        Some((summary, last_updated))
    }

    /// Make room for `wanted` more jobs in the bounded store, or answer the quota message
    /// a submit past the cap is refused with.
    ///
    /// Room is made by evicting the jobs that have been terminal longest — jobs no later
    /// call can move again — exactly as [`Self::insert_job`] does for a single job. An
    /// ARRAY asks for room for the whole of itself BEFORE any of it is built, so a submit
    /// that cannot fit is refused without first materialising the children it would have
    /// stored.
    pub fn make_room_for_jobs(&mut self, wanted: usize) -> Result<(), String> {
        let held = self.jobs.len();
        if held + wanted > limits::MAX_JOBS {
            self.evict_terminal_jobs(held + wanted - limits::MAX_JOBS);
        }
        if self.jobs.len() + wanted > limits::MAX_JOBS {
            return Err(quota_message("jobs", limits::MAX_JOBS));
        }
        Ok(())
    }

    /// Persist an array job: the `parent` and the `children` its submit spawned.
    ///
    /// The caller has already made room for the whole array
    /// ([`Self::make_room_for_jobs`]) — a half-created array would leave a parent whose
    /// children nothing can ever complete, and asking for the room BEFORE the children are
    /// built is what keeps one request's records bounded by the store's cap rather than by
    /// the size it named. Each insert still answers that bound for itself
    /// ([`Self::insert_job`]). The children go in in index order, so a `SEQUENTIAL` child's
    /// dependency on the index before it resolves as it is inserted.
    pub fn insert_array_job(&mut self, parent: Job, children: Vec<Job>) -> Result<(), String> {
        self.insert_job(parent)?;
        for child in children {
            self.insert_job(child)?;
        }
        Ok(())
    }

    /// Materialise the status the children of every array parent imply, at `now`.
    ///
    /// `ListJobs$jobStatus` states the rule for a parent: "Array job parents are updated to
    /// PENDING when any child job is updated to RUNNABLE and remain in PENDING status while
    /// child jobs are running. To view these jobs, filter by PENDING status until all child
    /// jobs reach a terminal state." So a parent leaves `SUBMITTED` for `PENDING` as soon as
    /// any child has left it, stays there while any child is live — it is never `RUNNABLE`,
    /// because it is never placed and so occupies no position in its queue's dispatch
    /// ordering — and becomes terminal when every child is: `SUCCEEDED` when they all
    /// succeeded and `FAILED` when any of them did not, at the instant the last of them
    /// ended.
    ///
    /// A terminal parent carries no `statusReason` of its own: AWS Batch sends one for the
    /// condition that ended a job, and what ended the parent is its children's outcomes,
    /// which the summary already reports. The one exception is a `CancelJob` the parent was
    /// given while it was `PENDING`, which `CancelJob` defers to exactly this point ("the
    /// array parent job is cancelled when all child jobs are completed"): the parent then
    /// records the reason that caller gave.
    ///
    /// Every instant written here is read off the children's own records, so what a later
    /// caller sees does not depend on when this ran.
    pub fn advance_array_parents(&mut self, now: i64) {
        let parents: Vec<String> = self
            .jobs
            .values()
            .filter(|job| job.array_size.is_some() && !is_terminal_status(&job.status))
            .map(|job| job.job_id.clone())
            .collect();
        for id in parents {
            let Some((status, at)) = self.array_parent_status(&id) else {
                continue;
            };
            if at > now {
                continue;
            }
            let cancelled = self
                .jobs
                .get(&id)
                .is_some_and(|parent| parent.cancel_requested.is_some());
            match (is_terminal_status(status), cancelled) {
                (true, true) => self.finish_cancelled(&id, at),
                (true, false) => self.finish_job(&id, status, None, at),
                (false, _) => {
                    if self.jobs.get(&id).is_some_and(|parent| parent.status != status) {
                        self.set_job_status(&id, status, at);
                        self.set_job_due(&id, None);
                    }
                }
            }
        }
    }

    /// The status the children of the array parent `id` imply, and the instant they implied
    /// it, or `None` for a job whose children this store holds none of.
    ///
    /// `PENDING` from the instant the FIRST child left `SUBMITTED`, the terminal pair from
    /// the instant the LAST child ended, and `SUBMITTED` reported as no change at all while
    /// every child is still there.
    fn array_parent_status(&self, id: &str) -> Option<(&'static str, i64)> {
        let children = self.array_children(id);
        if children.is_empty() {
            return None;
        }
        let mut ended_at = i64::MIN;
        let mut succeeded = true;
        let mut all_terminal = true;
        let mut left_submitted: Option<i64> = None;
        for child in children {
            if child.status != JOB_SUBMITTED {
                let at = child.status_since;
                left_submitted = Some(left_submitted.map_or(at, |first: i64| first.min(at)));
            }
            if is_terminal_status(&child.status) {
                succeeded &= child.status == JOB_SUCCEEDED;
                ended_at = ended_at.max(child.ended_at.unwrap_or(child.status_since));
            } else {
                all_terminal = false;
            }
        }
        if all_terminal {
            return Some((if succeeded { JOB_SUCCEEDED } else { JOB_FAILED }, ended_at));
        }
        left_submitted.map(|at| (JOB_PENDING, at))
    }

    /// The ids of the children this store holds for the array parent `id`, in index order,
    /// and none at all for every job that is not an array parent.
    ///
    /// The owned form of [`Self::array_children`], for a caller that goes on to WRITE each
    /// child.
    pub(super) fn array_child_ids(&self, id: &str) -> Vec<String> {
        self.array_children(id)
            .into_iter()
            .map(|child| child.job_id.clone())
            .collect()
    }

    /// Whether `job` is the child of an array parent this store still holds in a
    /// non-terminal status.
    ///
    /// Such a child is not evictable: the parent's status and summary are counted from its
    /// children's records, so one of them going would leave the parent with an array
    /// nothing could complete.
    pub(super) fn is_child_of_live_array_parent(&self, job: &Job) -> bool {
        job.array_parent_id.as_deref().is_some_and(|parent_id| {
            self.jobs
                .get(parent_id)
                .is_some_and(|parent| !is_terminal_status(&parent.status))
        })
    }
}
