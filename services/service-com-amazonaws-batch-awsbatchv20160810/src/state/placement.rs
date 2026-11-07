// SPDX-License-Identifier: CC0-1.0

//! Where a `RUNNABLE` job is PLACED, and what a placed job can report.
//!
//! `JobStatus$RUNNABLE` is the status in which "the job is ready to be scheduled to a
//! host", so what a job does next is decided by the compute capacity behind its queue and
//! not by the job: AWS Batch places a `RUNNABLE` job on a compute environment of its queue
//! that can hold it, and a queue whose environments offer the service no capacity to place
//! it on leaves the job waiting there (`JobDetail$status` points at "Jobs stuck in RUNNABLE
//! status" for exactly that case). `computeResources` is "details of the compute resources
//! managed by the compute environment" (`ComputeEnvironmentDetail$computeResources`), so a
//! `MANAGED` environment is the one whose capacity the SERVICE provisions and schedules
//! onto; an `UNMANAGED` one holds whatever instances the caller registered itself, of which
//! this emulator holds none. `CEState$DISABLED`: "the compute environment doesn't scale out"
//! and Batch "doesn't place jobs" on it, and an environment still `CREATING` has no capacity
//! yet.
//!
//! What a PLACED job can report is where the engine contract binds
//! (`vendor/engine/src/lib.rs`): containers are not available in-process, so the attempt a
//! placement starts is never run, and the job ends in the modelled failure state saying so
//! rather than reporting an exit code, a duration or an attempt for work nothing ran.

use super::*;

/// `statusReason` of a job the scheduler placed and whose container nothing ran.
///
/// A placed job's work is a container image, which this emulator has no runtime for and
/// the engine contract (`vendor/engine/src/lib.rs`) forbids pretending to run: so the run
/// the caller asked for ends in the modelled failure state saying exactly that, and no
/// `startedAt`, `stoppedAt`, attempt or `exitCode` is invented for it. Worded as
/// [`NO_SERVICE_JOB_RUNTIME_REASON`] is, naming the runtime that is missing.
pub const NO_CONTAINER_RUNTIME_REASON: &str =
    "This emulator does not run containers, so the job's container was never started";

impl ComAmazonawsBatchAwsbatchv20160810State {
    /// Whether the compute environments behind `id`'s queue offer the service capacity to
    /// place the job on.
    ///
    /// True when the queue orders at least one `MANAGED` compute environment that is
    /// `ENABLED` and past its own creation — the environment whose capacity the service
    /// itself provisions and schedules onto. An `UNMANAGED` environment (whose instances
    /// the caller registers, of which this store holds none), a `DISABLED` one (Batch
    /// places no job on it) and one still `CREATING` or `DELETING` offer none, so a queue
    /// with only those leaves its jobs waiting in `RUNNABLE`.
    ///
    /// An array PARENT is never placed: it runs no container of its own, and its status is
    /// what its children add up to ([`Self::advance_array_parents`]).
    ///
    /// Costs one walk of the queue's `computeEnvironmentOrder`, which
    /// `CreateJobQueue`/`UpdateJobQueue` bound, and reads nothing else.
    pub fn job_is_placeable(&self, id: &str) -> bool {
        let Some(job) = self.jobs.get(id) else {
            return false;
        };
        if job.array_size.is_some() {
            return false;
        }
        self.job_queue(&job.job_queue_arn).is_some_and(|queue| {
            queue.compute_environment_order.iter().any(|entry| {
                self.compute_environment(&entry.compute_environment)
                    .is_some_and(|environment| {
                        environment.ce_type == CE_TYPE_MANAGED
                            && environment.state == STATE_ENABLED
                            && matches!(
                                environment.status.as_str(),
                                STATUS_VALID | STATUS_UPDATING
                            )
                    })
            })
        })
    }

    /// Owe `id` its placement one span after `at`, or nothing at all when its queue has no
    /// capacity to place it on — in which case the job RESTS in `RUNNABLE`.
    ///
    /// Called from the ONE place a job enters `RUNNABLE`, so the decision is taken once
    /// per entry and from the instant the job entered, never from the instant a reader
    /// looked.
    pub(super) fn arm_placement(&mut self, id: &str, at: i64) {
        let due = self
            .job_is_placeable(id)
            .then_some(at + JOB_PHASE_MILLIS);
        self.set_job_due(id, due);
    }

    /// Owe the placement span again to every job resting in `RUNNABLE` whose queue the
    /// capacity published at `at` now lets it be placed on.
    ///
    /// A resting job owes nothing while its queue's compute environments offer no capacity,
    /// so what makes its next move due is the change that publishes some: a compute
    /// environment reaching the end of its creation, one a caller re-enabled
    /// (`UpdateComputeEnvironment`), or a queue given a different `computeEnvironmentOrder`
    /// (`UpdateJobQueue`). The placement then falls due one span after `at`, like every
    /// other transition this service owes — and `at` is the instant the capacity was
    /// published, so what a later caller sees does not depend on when it called. The twin
    /// of [`Self::rearm_resting_service_jobs`] for container jobs.
    pub fn rearm_resting_jobs(&mut self, at: i64) {
        let resting: Vec<String> = self
            .jobs
            .values()
            .filter(|job| job.status == JOB_RUNNABLE && job.next_phase_due_at.is_none())
            .map(|job| job.job_id.clone())
            .filter(|id| self.job_is_placeable(id))
            .collect();
        for id in resting {
            self.set_job_due(&id, Some(at + JOB_PHASE_MILLIS));
        }
    }
}
