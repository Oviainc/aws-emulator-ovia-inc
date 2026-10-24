Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconvert.queue-create-get-delete-lifecycle mediaconvert.delete-default-queue-rejected mediaconvert.cancel-already-canceled-job-conflict-rejected mediaconvert.create-job-from-template-inherits-output-settings mediaconvert.queue-pause-blocks-then-unpause-allows-job-progress mediaconvert.job-progresses-to-error-on-missing-input
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert.queue-create-get-delete-lifecycle|run#18|AssertionError|assert update_code == "NotFoundException", (
Issue-Key: mediaconvert.delete-default-queue-rejected|run#17|AssertionError|assert code3 == "NotFoundException", (
Issue-Key: mediaconvert.cancel-already-canceled-job-conflict-rejected|run#16|AssertionError|assert code is None, f"expected idempotent no-op re-cancel of a CANCELED job, got {code!r}"
Issue-Key: mediaconvert.create-job-from-template-inherits-output-settings|run#19|KeyError|output_groups = job["Settings"]["OutputGroups"]
Issue-Key: mediaconvert.queue-pause-blocks-then-unpause-allows-job-progress|run#21|AssertionError|assert paused_status == "SUBMITTED", f"job left SUBMITTED while its queue was PAUSED: {paused_status}"
Issue-Key: mediaconvert.job-progresses-to-error-on-missing-input|run#18|AssertionError|assert error_code == 1404, f"expected MediaConvert's input-probe-failure code 1404, got {error_code!r}"
Issue-Commit: 92ad2730926ec3062b662e938e8ed9f3fac68577

# MediaConvert job/queue plane: 5 fidelity divergences (UpdateQueue unimplemented, CancelJob not idempotent on a CANCELED job, CreateJob ignores JobTemplate Settings, PAUSED queue not honored, ERROR jobs carry no ErrorCode)

> _Note: this issue was identified against the code base at commit `92ad2730926ec3062b662e938e8ed9f3fac68577`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateQueue

## Description
This issue bundles 5 distinct net-new root causes in the MediaConvert emulator's job and queue plane. All 5 must be closed before submitting. (The open issue 20261022T040904Z-mediaconvert-queue-job-control-plane-4-fidelity-divergences.md covers four DIFFERENT causes — CreateQueue idempotency, CreateJob role-trust checking, the Default SYSTEM queue, and UpdateQueue's enum-rejection message. None of the causes below are covered by it: cause 1 here is the UpdateQueue operation not existing at all, which is a different defect from that issue's message-template cause.)

=== Cause 1 of 5: UpdateQueue is an unimplemented scaffold stub, so every queue mutation answers NotImplemented (HTTP 501) ===
AWS: Smithy: UpdateQueue is bound as PUT /2017-08-29/queues/{Name} with success code 200, input Name (@httpLabel, @required) plus Status (QueueStatus: ACTIVE|PAUSED), Description, ConcurrentJobs, MaximumConcurrentFeeds, ReservationPlanSettings; output Queue; modeled errors BadRequestException 400, ConflictException 409, ForbiddenException 403, NotFoundException 404, ServiceQuotaExceededException 402, TooManyRequestsException 429, InternalServerErrorException 500. Recorded behavior: UpdateQueue on an existing queue returns 200 with the mutated Queue, and UpdateQueue on a name that does not exist (including one just deleted) is rejected with NotFoundException (HTTP 404) — i.e. a mutation on a dead name is refused exactly like a read is. Recorded message, as an example of the request-derived template that names the member read: 'The resource you specified in name was not found.'
Emulator: services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/update_queue.rs is still the generated scaffold — its handle() returns Scaffolded::not_implemented("UpdateQueue"), so the operation answers HTTP 501 with error code 'NotImplemented' for every input. Both checks that mutate a queue observe 'NotImplemented' where they expect NotFoundException (404), and no queue can ever be paused/unpaused or re-described through the API.
Why: a client cannot pause, resume, resize or re-describe a queue at all, and its error handling sees a 501 that is in no modeled error set for this operation: NotFoundException-based control flow (treat a vanished queue as gone) never triggers, and retry logic that treats 501 as a server fault retries a request AWS would deterministically refuse with 404. It also blocks any scenario that needs a queue's Status changed (see cause 4).
Fix: implement the UpdateQueue handler (services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/update_queue.rs) against the queue store in src/state.rs — apply the supplied members to the named queue and return the updated Queue with 200, raise NotFoundException (404) for an unknown/deleted name consistently with GetQueue, and delete the SCAFFOLDED marker line.
Traces: mediaconvert.queue-create-get-delete-lifecycle, mediaconvert.delete-default-queue-rejected.

=== Cause 2 of 5: re-canceling an already-CANCELED job is rejected with ConflictException instead of succeeding idempotently ===
AWS: Smithy: CancelJob is DELETE /2017-08-29/jobs/{Id} with success code 202, empty output, and models ConflictException httpError 409 among its errors. Recorded behavior (the check states it was measured against real AWS, not guessed): CancelJob on a job that is ALREADY in the terminal CANCELED state succeeds again with no error — the call is idempotent — and the job's Status stays CANCELED; only the first call on a still-SUBMITTED job is the actual transition. So the terminal-state repeat is a no-op success (202), not a 409.
Emulator: the second CancelJob is rejected with ConflictException (409). crate::state::cancel_job returns CancelOutcome::NotCancelable for any job whose status is not SUBMITTED, and ops/cancel_job.rs maps that outcome to ConflictException (message example: 'Job <id> has status CANCELED and can no longer be canceled.'). CANCELED is treated identically to COMPLETE/ERROR/PROGRESSING.
Why: this is an over-permissive rejection of a retry-safe call. Any client that re-sends CancelJob after a timeout, or cancels in a cleanup/teardown path that may run twice (the behavioral checks' own on_cleanup does exactly this), gets a hard 409 where AWS returns success, so idempotent teardown code fails against the emulator and must grow conflict-swallowing it does not need on AWS.
Fix: CancelJob's state transition in src/state.rs (cancel_job) plus the ops/cancel_job.rs mapping — when the job is already CANCELED, return success (202) and leave Status/finish_time untouched; keep ConflictException only for jobs in a non-cancelable NON-CANCELED state.
Traces: mediaconvert.cancel-already-canceled-job-conflict-rejected.

=== Cause 3 of 5: CreateJob with a JobTemplate stores the raw request Settings and never merges the template's settings ===
AWS: Smithy: CreateJob input carries JobTemplate (__string, jsonName 'jobTemplate') alongside the @required Settings (JobSettings); the Job output shape carries both JobTemplate ('The job template that the job is created from, if it is created from a job template.') and Settings. Recorded behavior: when CreateJob names a JobTemplate and supplies only Inputs in Settings, the created job's effective Settings are the merge of the template's settings with the request's — GetJob on the new job returns Settings.OutputGroups inherited from the template (the template's output group name, its FileGroupSettings.Destination and its per-output ContainerSettings), together with the request's Inputs, and Job.JobTemplate names the template.
Emulator: ops/create_job.rs builds the JobRecord with `settings: input.settings` and `job_template: input.job_template` and never resolves the template against state.job_templates, so nothing is merged. The stored job's Settings is exactly the request body, and GetJob returns Settings with only 'Inputs' — the check fails with KeyError 'OutputGroups' reading job['Settings']['OutputGroups'].
Why: a client that keeps its encoding configuration in job templates (the normal MediaConvert pattern — submit only the input, inherit all outputs) reads back a job with no outputs at all, so anything that inspects the created job's outputs (validation, cost estimation, downstream path construction from FileGroupSettings.Destination) sees an empty/wrong configuration that AWS would have populated.
Fix: the CreateJob handler (services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/create_job.rs) — resolve the JobTemplate reference against the job-template store in src/state.rs and merge the template's settings under the request's Settings (request members win, template supplies what the request omits, notably OutputGroups) before the record is persisted, so GetJob/ListJobs project the merged settings.
Traces: mediaconvert.create-job-from-template-inherits-output-settings.

=== Cause 4 of 5: a job on a PAUSED queue is advanced out of SUBMITTED anyway; queue Status is not consulted ===
AWS: Smithy: QueueStatus is the enum ACTIVE|PAUSED and its documentation states the contract directly — 'Queues can be ACTIVE or PAUSED. If you pause a queue, jobs in that queue won't begin. Jobs that are running when you pause a queue continue to run until they finish or result in an error' (UpdateQueue.Status repeats it). Recorded behavior: a job submitted to a PAUSED queue stays SUBMITTED for as long as the queue is paused — it is still SUBMITTED after an identical control job on an ACTIVE queue has already left SUBMITTED — and it leaves SUBMITTED only once the queue is set back to ACTIVE.
Emulator: the job on the PAUSED queue leaves SUBMITTED on a timer regardless of its queue: ComAmazonawsMediaconvertMediaconvertState::settle (src/state.rs) drains the `pending` set purely by due instant and flips every still-SUBMITTED job to ERROR without ever looking up the job's queue or that queue's status. The check observes the paused job in status ERROR while its queue is PAUSED.
Why: the PAUSED queue is a no-op, so the one control a client has to hold work back is ineffective — jobs submitted to a deliberately paused queue (staged release, cost gating, maintenance windows) start and in this emulator even terminate, and the job state a client reads diverges from AWS's for the entire paused window. Compounded by cause 1, a paused queue cannot even be re-activated.
Fix: the job-progression logic in src/state.rs (settle / the `pending` index) — gate a job's exit from SUBMITTED on its queue's Status being ACTIVE, leaving jobs on a PAUSED queue in SUBMITTED (and re-arming them when the queue is set ACTIVE again, once cause 1 makes that reachable).
Traces: mediaconvert.queue-pause-blocks-then-unpause-allows-job-progress.

=== Cause 5 of 5: a job that ends in ERROR carries no ErrorCode and a canned ErrorMessage that does not name the failing input ===
AWS: Smithy: Job.ErrorCode is an __integer ('Error code for the job') and Job.ErrorMessage is a __string ('Error message of Job'); JobStatus includes ERROR. Recorded behavior: a job whose S3 input cannot be probed/opened (the input bucket does not exist) reaches terminal ERROR with ErrorCode 1404 — MediaConvert's input-probe-failure code — and an ErrorMessage that names the specific input URI this request submitted.
Emulator: the job does reach ERROR, but GetJob reports ErrorCode absent (None) and an ErrorMessage that is the same canned string for every job: src/state.rs settle() sets error_message = NO_TRANSCODER_MESSAGE unconditionally, and JobRecord::view() hardcodes error_code: None, so the failure is neither classified nor tied to the request's Settings.Inputs.
Why: ErrorCode is how clients branch on failure — retry/alert on an input-access failure (1404) versus a settings or output failure — so an absent ErrorCode makes every failed job indistinguishable, and a generic ErrorMessage that never names the offending input gives operators nothing to act on. Code that reads Job.ErrorCode as an int also faces a missing key where AWS always supplies one on an ERROR job.
Fix: the job-progression logic and the Job projection in src/state.rs — when a job fails because an input cannot be read, set ErrorCode to MediaConvert's input-probe-failure code 1404 and build ErrorMessage from the job's own Settings.Inputs[].FileInput so it names the input that failed; project error_code from the record in JobRecord::view() instead of hardcoding None.
Traces: mediaconvert.job-progresses-to-error-on-missing-input.

## Evidence
Suite: simparity-behavioral-aws-v1, mediaconvert 8/21 (38%), 13 failed.

Cause 1 — mediaconvert.queue-create-get-delete-lifecycle [mismatch]
  Expected: after delete_queue(Name=N), update_queue(Name=N, Description='post-delete') raises NotFoundException (404).
  Actual: AssertionError: expected NotFoundException for UpdateQueue on deleted queue, got 'NotImplemented'.
Cause 1 — mediaconvert.delete-default-queue-rejected [mismatch]
  Expected: a mutation on an already-deleted queue also fails NotFoundException (404).
  Actual: AssertionError: expected a mutation on the already-gone queue to also fail NotFoundException, got 'NotImplemented'.
  Source: ops/update_queue.rs still carries the '//! SCAFFOLDED, NOT IMPLEMENTED' marker and returns Scaffolded::not_implemented("UpdateQueue").

Cause 2 — mediaconvert.cancel-already-canceled-job-conflict-rejected [mismatch]
  Expected: cancel_job(Id=J) on a job already CANCELED raises nothing (idempotent 202) and GetJob still reports Status 'CANCELED'.
  Actual: AssertionError: expected idempotent no-op re-cancel of a CANCELED job, got 'ConflictException'. (state.rs cancel_job -> CancelOutcome::NotCancelable for any non-SUBMITTED status; ops/cancel_job.rs maps it to ConflictException 409.)

Cause 3 — mediaconvert.create-job-from-template-inherits-output-settings [error]
  Expected: create_job(JobTemplate=T, Settings={'Inputs':[...]}) then get_job -> Job.Settings.OutputGroups == the template's one 'File Group' (FILE_GROUP_SETTINGS destination, one MP4 output).
  Actual: KeyError: 'OutputGroups' at job['Settings']['OutputGroups'] — the stored Settings is the raw request body (Inputs only). ops/create_job.rs: `settings: input.settings` with no template resolution.

Cause 4 — mediaconvert.queue-pause-blocks-then-unpause-allows-job-progress [mismatch]
  Expected: while its queue is PAUSED the job's Status stays 'SUBMITTED' for at least as long as an identical control job on an ACTIVE queue needs to leave SUBMITTED.
  Actual: AssertionError: job left SUBMITTED while its queue was PAUSED: ERROR. (state.rs settle() flips every due SUBMITTED job to ERROR without consulting the job's queue status.)

Cause 5 — mediaconvert.job-progresses-to-error-on-missing-input [mismatch]
  Expected: job reaches ERROR with Job.ErrorCode == 1404 and an ErrorMessage naming the submitted input URI.
  Actual: Status ERROR is reached, but AssertionError: expected MediaConvert's input-probe-failure code 1404, got None. (state.rs: view() hardcodes error_code: None; settle() sets the canned NO_TRANSCODER_MESSAGE.)

Model citations used: Smithy UpdateQueue (PUT /2017-08-29/queues/{Name}, 200, Name @httpLabel @required, Status -> QueueStatus, errors incl. NotFoundException 404), CancelJob (DELETE /2017-08-29/jobs/{Id}, 202, ConflictException 409), CreateJob (POST /2017-08-29/jobs, 201, JobTemplate jsonName 'jobTemplate', Settings @required), GetJob (GET /2017-08-29/jobs/{Id}, 200), Job (ErrorCode __integer, ErrorMessage __string, JobTemplate, Settings, Status -> JobStatus SUBMITTED|PROGRESSING|COMPLETE|CANCELED|ERROR), QueueStatus (ACTIVE|PAUSED, doc: paused queues' jobs won't begin).

## Steps to Reproduce
Boot the emulator and point a botocore mediaconvert client at it (region us-west-2).

Cause 1: create_queue(Name=N); delete_queue(Name=N); call update_queue(Name=N, Description='post-delete'). Expect ClientError NotFoundException (404); observe error code 'NotImplemented' (HTTP 501). Same for update_queue on any existing queue.

Cause 2: create_queue(Name=Q, Status='PAUSED'); create_job(Role=<role trusting mediaconvert.amazonaws.com>, Queue=Q, Settings={'Inputs':[{'FileInput':'s3://b/in.mp4'}],'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}); cancel_job(Id=<job id>); poll get_job until Status == 'CANCELED'; call cancel_job(Id=<same id>) again. Expect success with Status still 'CANCELED'; observe ConflictException.

Cause 3: create_job_template(Name=T, Settings={'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}); create_queue(Name=Q, Status='PAUSED'); create_job(Role=R, Queue=Q, JobTemplate=T, Settings={'Inputs':[{'FileInput':'s3://b/input.mp4'}]}); get_job(Id=<job id>). Expect Job.JobTemplate == T and Job.Settings.OutputGroups == the template's one group; observe Job.Settings with only 'Inputs'.

Cause 4: create_queue(Name=P, Status='PAUSED') and create_queue(Name=A) (ACTIVE). Submit the same job to each. Poll both: the ACTIVE-queue job leaves SUBMITTED; while it is still SUBMITTED the PAUSED-queue job must also be SUBMITTED, and must remain SUBMITTED after it. Observe the PAUSED-queue job in 'ERROR'.

Cause 5: create_queue(Name=Q) (ACTIVE); create_job(Role=R, Queue=Q, Settings pointing at s3://<bucket-that-does-not-exist>/does-not-exist.mp4); poll get_job until Status is terminal. Expect Status 'ERROR' with ErrorCode 1404 and an ErrorMessage naming that input URI; observe Status 'ERROR' with ErrorCode absent and a generic message that does not name the input.
