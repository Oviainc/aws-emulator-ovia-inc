Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: deadline.create-job-target-task-run-status-suspended deadline.create-job-lifecycle-create-complete
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline.create-job-target-task-run-status-suspended|run#9|KeyError|assert job["taskRunStatus"] == "SUSPENDED", (
Issue-Key: deadline.create-job-lifecycle-create-complete|_poll_until#12|Timeout|time.sleep(interval_s)
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# deadline GetJob never reports taskRunStatus and a job on a fleet-less queue is never reconciled, so the submitted targetTaskRunStatus and the no-fleet settle are both unobservable (2 causes)

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
Two net-new root causes in deadline's job plane, both rooted in GetJob reporting no run status and the stored job's lifecycle only ever advancing one step.

Cause 1 of 2 — GetJob omits taskRunStatus entirely, so a job created with targetTaskRunStatus=SUSPENDED reports no run status at all.
AWS: Smithy: CreateJob.targetTaskRunStatus targets CreateJobTargetTaskRunStatus (READY | SUSPENDED, default READY); GetJob's output carries taskRunStatus (TaskRunStatus: PENDING | READY | ASSIGNED | STARTING | SCHEDULED | INTERRUPTING | RUNNING | SUSPENDED | CANCELED | FAILED | SUCCEEDED | NOT_COMPATIBLE) and targetTaskRunStatus (JobTargetTaskRunStatus), both optional in the model. Recorded behavior: real GetJob surfaces targetTaskRunStatus only while lifecycleStatus is CREATE_IN_PROGRESS (it is the goal creation converges to); once lifecycleStatus is CREATE_COMPLETE the member is absent and taskRunStatus carries the settled state — SUSPENDED for a job submitted with targetTaskRunStatus=SUSPENDED. taskRunStatus is the durable signal that the submitted request was honored.
Emulator: GetJob hardcodes taskRunStatus absent and always reports targetTaskRunStatus — services/service-com-amazonaws-deadline-deadline/src/ops/get_job.rs:227-230 sets `task_run_status: None, target_task_run_status: Some(job.target_task_run_status.clone()), task_run_status_counts: None, task_failure_retry_count: None` with a comment that the slice persists no steps or tasks. The reply is an HTTP 200 whose body simply lacks the member, so the SDK raises KeyError: 'taskRunStatus'.
Why: a client cannot confirm that a job was submitted suspended (nor read a job's run state at all) — the one field that reports it is never served, and the field that IS served (targetTaskRunStatus after CREATE_COMPLETE) is one AWS has already dropped by then, so a client keying off its presence takes the wrong branch.
Fix: the job record and its read projection — persist a run status on crate::state::Job (src/state.rs:129-160, initialized from CreateJob's targetTaskRunStatus) and report it from src/ops/get_job.rs, dropping targetTaskRunStatus once lifecycleStatus is no longer CREATE_IN_PROGRESS.
Traces: deadline.create-job-target-task-run-status-suspended.

Cause 2 of 2 — a job whose queue has no fleet association is never reconciled: it parks at CREATE_COMPLETE forever instead of settling to lifecycleStatus=UPDATE_SUCCEEDED / taskRunStatus=NOT_COMPATIBLE.
AWS: recorded behavior only (the model lists JobLifecycleStatus UPDATE_SUCCEEDED and TaskRunStatus NOT_COMPATIBLE but documents no reconciliation rule). Recording: a job submitted to a queue with no fleet associated has no route to ever run; AWS does not leave it at the submitted default — asynchronously (within the check's 60 s window, briefly passing through a transient state carrying the submitted targetTaskRunStatus) it settles to taskRunStatus=NOT_COMPATIBLE and lifecycleStatus=UPDATE_SUCCEEDED, while priority persists exactly as submitted (42).
Emulator: the only clock-owed transition the crate models is CREATE_IN_PROGRESS -> CREATE_COMPLETE after 500 ms (src/state.rs:30-39 JOB_CREATE_COMPLETE_AFTER and Job::settle_lifecycle, src/state.rs:162-177, which sets only CREATE_COMPLETE and the message "The job was created and is ready to be scheduled."). There is no further owed transition, and taskRunStatus is never populated (cause 1), so a poll for a settled taskRunStatus never completes and the check is cut off at its 60 s timeout.
Why: a client that waits for a job to reach a terminal/settled state against this emulator hangs forever, and a client asserting that an unschedulable job is reported NOT_COMPATIBLE instead of runnable sees the opposite — the emulator leaves it looking freshly created and ready.
Fix: the job lifecycle in src/state.rs — extend the clock-owed transition machinery (Job::create_complete_due_at / settle_lifecycle) with the no-fleet reconciliation that moves a job on a queue with no fleet association to lifecycleStatus UPDATE_SUCCEEDED and taskRunStatus NOT_COMPATIBLE, materialized on read exactly as the CREATE_COMPLETE move already is; GetJob (src/ops/get_job.rs) then just reports it.
Traces: deadline.create-job-lifecycle-create-complete.

## Evidence
Suite: 0/22 deadline checks passed.

[error] deadline.create-job-target-task-run-status-suspended — after CreateJob(targetTaskRunStatus='SUSPENDED') and polling GetJob to lifecycleStatus=CREATE_COMPLETE: `assert job["taskRunStatus"] == "SUSPENDED"` -> KeyError: 'taskRunStatus'. Expected: taskRunStatus='SUSPENDED' present and targetTaskRunStatus absent once CREATE_COMPLETE. Actual: taskRunStatus absent from the 200 body, targetTaskRunStatus still present.

[mismatch] deadline.create-job-lifecycle-create-complete — Check timed out after 60.0s while polling GetJob for `taskRunStatus in (NOT_COMPATIBLE, CANCELED, FAILED, SUCCEEDED)`. Expected: taskRunStatus=NOT_COMPATIBLE and lifecycleStatus=UPDATE_SUCCEEDED (priority 42 preserved) on a queue with no fleet. Actual: the job stops at lifecycleStatus=CREATE_COMPLETE with no taskRunStatus ever served.

Code evidence: src/ops/get_job.rs:225-230 (`task_run_status: None` with the comment "This slice persists no steps or tasks … these stay absent until the step/task operations are written"); src/state.rs:162-177 Job::settle_lifecycle only applies CREATE_IN_PROGRESS -> CREATE_COMPLETE; crate::state::Job (src/state.rs:129-160) stores target_task_run_status but no run status.

Model evidence: GetJob output members taskRunStatus (TaskRunStatus) and targetTaskRunStatus (JobTargetTaskRunStatus); TaskRunStatus includes SUSPENDED and NOT_COMPATIBLE; JobLifecycleStatus includes UPDATE_SUCCEEDED; CreateJobTargetTaskRunStatus = READY | SUSPENDED (default READY).

## Steps to Reproduce
1. deadline.create_farm(displayName=...) -> farmId; deadline.create_queue(farmId=..., displayName=...) -> queueId (no fleet is associated with this queue).
2. deadline.create_job(farmId=..., queueId=..., template=<minimal OpenJD jobtemplate-2023-09 JSON>, templateType='JSON', priority=10, targetTaskRunStatus='SUSPENDED') -> jobId.
3. Poll deadline.get_job(farmId=..., queueId=..., jobId=...) until lifecycleStatus == 'CREATE_COMPLETE', then read taskRunStatus.
   Observed: the key is absent (KeyError) and targetTaskRunStatus is still reported. Expected: taskRunStatus == 'SUSPENDED', targetTaskRunStatus absent.
4. Repeat steps 1-2 with priority=42 and no targetTaskRunStatus, then poll get_job for up to 60 s for taskRunStatus in (NOT_COMPATIBLE, CANCELED, FAILED, SUCCEEDED).
   Observed: the poll never completes — lifecycleStatus stays CREATE_COMPLETE and taskRunStatus is never present. Expected: taskRunStatus == 'NOT_COMPATIBLE' and lifecycleStatus == 'UPDATE_SUCCEEDED', with priority still 42.
