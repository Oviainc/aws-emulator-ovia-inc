Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconvert-queue-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconvert-queue-lifecycle mediaconvert-job-template-lifecycle mediaconvert-preset-lifecycle mediaconvert-job-submission-workflow
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|CreateQueue|2xx->4xx|$.queue
Issue-Key: mediaconvert|CreateQueue|2xx->4xx|status
Issue-Key: mediaconvert|CreateJobTemplate|2xx->4xx|$.jobTemplate
Issue-Key: mediaconvert|CreateJobTemplate|2xx->4xx|status
Issue-Key: mediaconvert|CreatePreset|2xx->4xx|$.preset
Issue-Key: mediaconvert|CreatePreset|2xx->4xx|status
Issue-Key: mediaconvert|DeleteQueue|2xx->4xx|status
Issue-Key: mediaconvert|CancelJob|2xx->4xx|$
Issue-Key: mediaconvert|CancelJob|2xx->4xx|status
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# MediaConvert repeat create/delete/cancel rejected instead of idempotent success

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQueue

## Description
This issue bundles 5 distinct net-new WIRE root causes with one shared fault: the MediaConvert emulator rejects a REPEATED request that AWS answers with idempotent success. In each case the recorded response is a success status and the emulator returns a 4xx conflict/not-found instead. All 5 must be closed before submitting.

Relation to open issues: 20261022T040904Z-mediaconvert-queue-job-control-plane-4-fidelity-divergences.md (cause 1) and 20261022T110844Z-mediaconvert-job-queue-plane-5-fidelity-divergences-updatequeue.md (cause 2) describe CreateQueue idempotency and CancelJob re-cancel from the BEHAVIORAL suite; neither carries the wire replay keys below, so none of these clusters is owned by them (the replay reports owned_by=[] for all five). Causes 2, 3 and 4 here (CreateJobTemplate, CreatePreset, DeleteQueue) are not covered by any open issue at all.

=== Cause 1 of 5: CreateQueue with an existing Name returns 409 ConflictException instead of 201 with the existing queue ===
AWS: Smithy: CreateQueue is POST /2017-08-29/queues with success code 201, Name @required (jsonName 'name'), output Queue; ConflictException is merely one of the modeled errors (httpError 409). Recorded behavior only: a second CreateQueue with the SAME Name and the same body succeeds with HTTP 201 and returns the EXISTING queue (same arn, the original description 'probe queue for lifecycle test', type CUSTOM, pricingPlan ON_DEMAND, status ACTIVE) - the create is idempotent on Name.
Emulator: HTTP 409 with ConflictException and no queue body at all ($.queue missing).
Why: provisioning code that re-runs (retry after a timed-out-but-applied create, two workers bootstrapping the same queue, idempotent infrastructure scripts) gets a hard failure where AWS returns the resource, so setup that is safe on AWS is not re-runnable against the emulator and must grow conflict-swallowing it does not need.
Fix: the CreateQueue handler plus the queue store - on an existing queue with the same Name, return 201 with the existing queue's projection instead of raising ConflictException, and keep the existence check and insert atomic.
Traces: mediaconvert-queue-lifecycle#task#4.

=== Cause 2 of 5: CreateJobTemplate with an existing Name returns 409 ConflictException instead of 201 with the existing template ===
AWS: Smithy: CreateJobTemplate is POST /2017-08-29/jobTemplates with success code 201, Name @required, Settings @required, output JobTemplate; ConflictException httpError 409 is modeled but not required here. Recorded behavior only: re-creating the same Name succeeds with 201 and returns the existing template (same arn, description 'probe job template', type CUSTOM, its stored settings, accelerationSettings {mode DISABLED}, statusUpdateInterval SECONDS_60, priority 0, hopDestinations []).
Emulator: HTTP 409 and no $.jobTemplate in the body.
Why: the same re-runnability break on the template plane - a client that declares its templates idempotently on every deploy fails on the second deploy.
Fix: the CreateJobTemplate handler plus the job-template store - return the existing template with 201 on a duplicate Name.
Traces: mediaconvert-job-template-lifecycle#task#4.

=== Cause 3 of 5: CreatePreset with an existing Name returns 409 ConflictException instead of 201 with the existing preset ===
AWS: Smithy: CreatePreset is POST /2017-08-29/presets with success code 201, Name @required, Settings @required, output Preset; ConflictException httpError 409 modeled. Recorded behavior only: re-creating the same Name succeeds with 201 and returns the existing preset (same arn, description 'probe preset', category, type CUSTOM, its stored settings).
Emulator: HTTP 409 and no $.preset in the body.
Why: as causes 1-2, on the preset plane.
Fix: the CreatePreset handler plus the preset store - return the existing preset with 201 on a duplicate Name.
Traces: mediaconvert-preset-lifecycle#task#4.

=== Cause 4 of 5: DeleteQueue on a name that no longer exists returns 404 NotFoundException instead of 202 ===
AWS: Smithy: DeleteQueue is DELETE /2017-08-29/queues/{Name} (Name @httpLabel @required) with success code 202 and an EMPTY output; NotFoundException httpError 404 is modeled. Recorded behavior only: after the queue is gone (the preceding GetQueue on the same name records 404), DeleteQueue on that same name still answers 202 with an empty body - the delete is idempotent even though the read is not.
Emulator: HTTP 404 (NotFoundException) for the repeat delete.
Why: teardown and cleanup paths run more than once (a retried teardown, a cleanup hook that also runs on the happy path). On AWS the second delete is a no-op success; against the emulator it raises, so teardown code that is correct on AWS fails here, and a client cannot distinguish 'already gone' from a real error without special-casing.
Fix: the DeleteQueue handler - when the named queue is absent, return the empty 202 success instead of NotFoundException (keeping ForbiddenException for SYSTEM queues and leaving GetQueue's 404 behavior unchanged).
Traces: mediaconvert-queue-lifecycle#teardown#14.

=== Cause 5 of 5: CancelJob on an already-CANCELED job returns 409 ConflictException instead of 202 ===
AWS: Smithy: CancelJob is DELETE /2017-08-29/jobs/{Id} (Id @httpLabel @required) with success code 202 and an EMPTY output; ConflictException httpError 409 is modeled. Recorded behavior only: cancelling a job that is already in the terminal CANCELED state succeeds again with 202 and an empty body - the repeat is a no-op, not a conflict.
Emulator: HTTP 409 with body {'__type': 'ConflictException', 'message': 'Job <id> has status CANCELED and can no longer be canceled.'} (message quoted as an EXAMPLE; the requirement is the 202 with an empty body).
Why: a client that re-sends CancelJob after a timeout, or cancels in a cleanup path that may run twice, gets a hard 409 where AWS returns success, so idempotent teardown fails against the emulator.
Fix: the CancelJob state transition and its outcome mapping - treat an already-CANCELED job as success (202, Status left CANCELED), reserving ConflictException for jobs in a non-cancelable NON-CANCELED state (COMPLETE/ERROR).
Traces: mediaconvert-job-submission-workflow#task#10.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator: 62 steps, 27 passed, score 43.5%. bootstrap_signal=false, emulator_up=true, state_reset_failures=0, so each cluster below is a genuine per-step divergence.

Cause 1 - mediaconvert-queue-lifecycle#task#4, CreateQueue [mismatch]
  Expected: status 201 with $.queue == {arn 'arn:aws:mediaconvert:<region>:<account>:queues/<name>', createdAt, lastUpdated, type 'CUSTOM', pricingPlan 'ON_DEMAND', status 'ACTIVE', description 'probe queue for lifecycle test', name <same name>, submittedJobsCount 0, progressingJobsCount 0, concurrentJobs 200, maximumConcurrentFeeds 0}
  Actual:   status 409, $.queue missing (ConflictException).

Cause 2 - mediaconvert-job-template-lifecycle#task#4, CreateJobTemplate [mismatch]
  Expected: status 201 with $.jobTemplate == the existing template (arn .../jobTemplates/<name>, description 'probe job template', type 'CUSTOM', its settings, accelerationSettings {'mode':'DISABLED'}, statusUpdateInterval 'SECONDS_60', priority 0, hopDestinations [])
  Actual:   status 409, $.jobTemplate missing.

Cause 3 - mediaconvert-preset-lifecycle#task#4, CreatePreset [mismatch]
  Expected: status 201 with $.preset == the existing preset (arn .../presets/<name>, description 'probe preset', category 'probe-category', type 'CUSTOM', its settings)
  Actual:   status 409, $.preset missing.

Cause 4 - mediaconvert-queue-lifecycle#teardown#14, DeleteQueue [mismatch]
  Expected: status 202 (empty body) for a delete of an already-absent queue - the immediately preceding recorded step, mediaconvert-queue-lifecycle#teardown#13 GetQueue on the same name, is a 404.
  Actual:   status 404.

Cause 5 - mediaconvert-job-submission-workflow#task#10, CancelJob [mismatch]
  Expected: status 202 with body {}
  Actual:   status 409 with {'__type': 'ConflictException', 'message': 'Job <id> has status CANCELED and can no longer be canceled.'}

Model citations used: Smithy CreateQueue (POST /2017-08-29/queues, 201, Name @required, output Queue; errors BadRequestException 400 / ConflictException 409 / ForbiddenException 403 / NotFoundException 404 / ServiceQuotaExceededException 402 / TooManyRequestsException 429 / InternalServerErrorException 500); CreateJobTemplate (POST /2017-08-29/jobTemplates, 201, Name+Settings @required); CreatePreset (POST /2017-08-29/presets, 201, Name+Settings @required); DeleteQueue (DELETE /2017-08-29/queues/{Name}, 202, empty output); CancelJob (DELETE /2017-08-29/jobs/{Id}, 202, empty output, ConflictException 409). That these repeats SUCCEED is recorded behavior only - the model permits a conflict but the recording shows AWS does not use one.

## Steps to Reproduce
Boot the emulator and point a botocore mediaconvert client at it (region us-west-2).

Cause 1: create_queue(Name=Q, Description='probe queue for lifecycle test'); call create_queue(Name=Q, Description='probe queue for lifecycle test') again. Expect HTTP 201 and the existing Queue (same Arn); observe ConflictException (409).

Cause 2: create_job_template(Name=T, Description='probe job template', Settings={'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}); call the identical create_job_template again. Expect 201 with the existing JobTemplate; observe 409.

Cause 3: create_preset(Name=P, Description='probe preset', Category='probe-category', Settings={'ContainerSettings':{'Container':'MP4'},'VideoDescription':{...H_264 QVBR...}}); call the identical create_preset again. Expect 201 with the existing Preset; observe 409.

Cause 4: create_queue(Name=Q2); delete_queue(Name=Q2); confirm get_queue(Name=Q2) raises NotFoundException (404); then call delete_queue(Name=Q2) again. Expect HTTP 202 with an empty body; observe 404.

Cause 5: create_queue(Name=Q3); create_job(Role=R, Queue=Q3, Settings={'Inputs':[{'FileInput':'s3://b/in.mp4'}],'OutputGroups':[{FILE_GROUP_SETTINGS -> 's3://b/out/', one MP4/H_264 output}]}); cancel_job(Id=<job id>); poll get_job until Status == 'CANCELED'; call cancel_job(Id=<same id>) again. Expect HTTP 202 with an empty body and Status still CANCELED; observe 409 ConflictException.
