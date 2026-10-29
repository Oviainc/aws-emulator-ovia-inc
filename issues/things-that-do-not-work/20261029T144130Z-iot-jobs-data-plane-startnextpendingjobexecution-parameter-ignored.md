Issue-Severity: MEDIUM
Issue-Gap-Class: parameter-ignored
Issue-Commit: 58d3cdd8a54717975c7a5e45d2c60ee5b89f816e

# A $next poll that reaches an already-IN_PROGRESS execution silently drops the request's statusDetails and stepTimeoutInMinutes

_Filed: 2026-10-29T14:41:30Z_
_Operation: StartNextPendingJobExecution | Class: parameter-ignored_

> _Note: this issue was identified against the code base at commit `58d3cdd8a54717975c7a5e45d2c60ee5b89f816e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`StartNextPendingJobExecution` validates `statusDetails` and `stepTimeoutInMinutes` up front, then applies them ONLY on the QUEUED -> IN_PROGRESS branch. When the execution the poll resolves to is already `IN_PROGRESS` — which the operation's own documentation lists as a pending status it serves — the handler skips the whole write block: the supplied `statusDetails` map is thrown away and the supplied step timeout arms no timer at all.

Both members are documented as request-driven, with exactly ONE stated condition for leaving the stored value alone. `statusDetails` says the details are unchanged "If not specified" — the handler adds a second, undocumented condition (the execution is already in progress) under which a SPECIFIED map is also unchanged. `stepTimeoutInMinutes` says it "Specifies the amount of time this device has to finish execution of this job", and the model declares its valid range (1..10080) — the handler accepts the value, spends a validation branch refusing it when it is out of range, and then stores nothing, so a device that polls `$next` after a restart (the normal way a device re-acquires work it already holds) is handed the job with NO step timer and no way to record its progress details through this operation.

The drop is silent: the response is 200 and carries the execution, so the caller cannot tell that the two members it sent were discarded. Note this is NOT the clock-driven case already fixed — the execution here is live and non-terminal (timers due were materialised one statement earlier).

## Evidence

The model, `StartNextPendingJobExecutionRequest$statusDetails` (smithy_query(query="doc", shape="StartNextPendingJobExecutionRequest", member="statusDetails")):

> "A collection of name/value pairs that describe the status of the job execution. If not specified, the statusDetails are unchanged."

The model, `StartNextPendingJobExecutionRequest$stepTimeoutInMinutes`:

> "Specifies the amount of time this device has to finish execution of this job. If the job execution status is not set to a terminal state before this timer expires, or before the timer is reset (by calling UpdateJobExecution, setting the status to IN_PROGRESS, and specifying a new timeout value in field stepTimeoutInMinutes) the job execution status will be automatically set to TIMED_OUT. ... Valid values for this parameter range from 1 to 10080 (1 minute to 7 days)."

The operation doc, which puts an already-started execution in scope:

> "Gets and starts the next pending (status IN_PROGRESS or QUEUED) job execution for a thing."

The handler drops both members on that path — services/service-com-amazonaws-iotjobsdataplane-iotlaserthingjobmanagerexternalservice/src/ops/start_next_pending_job_execution.rs:248-275:
```
            // An execution the thing already has IN_PROGRESS is merely re-handed:
            // this call starts nothing, so it writes nothing — the device gets the
            // same version, the same `startedAt` and the same details it was handed
            // before, ...
            if record.status != STATUS_IN_PROGRESS {
                ...
                if let Err(reason) = record.try_replace_status_details(input.status_details.as_ref())
                ...
                record.start(now);
                record.arm_step_timer(input.step_timeout_in_minutes, now);
                record.last_updated_at = now;
                record.version_number += 1;
            }
```
The same handler spends two validation branches on the members it then discards — start_next_pending_job_execution.rs:184-212 (`stepTimeoutInMinutes must be between {MIN_STEP_TIMEOUT_MINUTES} and {MAX_STEP_TIMEOUT_MINUTES}` and `crate::state::check_status_details(input.status_details.as_ref())`), so an out-of-range value is refused 400 while an in-range one is accepted and ignored.

Test coverage: the current behaviour is pinned by `re_handing_an_in_progress_execution_writes_nothing_and_the_queue_waits` (tests/all/job_execution_state_machine.rs:529), whose rows include "a re-poll carrying details" (line 561-563), "a re-poll carrying a step timeout" (564-567) and "a re-poll carrying both" (568-571), asserting at line 593 "re-handing an already-started execution must change nothing". Its stated reason (tests/all/job_execution_state_machine.rs:526-527, "none of which may change an execution this call does not start") is the crate's own invention — the file header says the rules are "asserted over a SET of inputs rather than one recorded case" (line 5) — and it is what the quoted member documentation contradicts: the model conditions "unchanged" on the member NOT being specified, not on the execution's current status. Fixing the handler requires updating those rows.

## Steps to Reproduce

1. Make a thing `laser-1` hold one QUEUED job execution (e.g. POST /command-executions with a registered `commandArn` targeting `arn:aws:iot:us-east-1:111122223333:thing/laser-1`).
2. PUT /things/laser-1/jobs/$next with body `{}` — 200, the execution is started (status IN_PROGRESS, versionNumber 2).
3. PUT /things/laser-1/jobs/$next again with body `{"statusDetails": {"stage": "download"}, "stepTimeoutInMinutes": 7}` — 200, the same execution is returned.
4. Observe the returned `execution` carries no `statusDetails` for `stage` and no `approximateSecondsBeforeTimedOut` from the 7-minute timer; the request's two members were accepted and discarded.
5. GET /things/laser-1/jobs/<jobId> and observe the stored execution likewise holds neither the supplied details nor an armed step timer, so the execution will never reach TIMED_OUT despite the device asking for a 7-minute step timeout.
