Issue-Source: trace-sweep-divergence
Issue-Trace: iot-job-continuous-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-job-continuous-lifecycle iot-job-list-lifecycle iot-command-duplicate-and-execution-errors
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreateJob|2xx->4xx|$.description
Issue-Key: iot|CreateJob|2xx->4xx|$.jobArn
Issue-Key: iot|CreateJob|2xx->4xx|$.jobId
Issue-Key: iot|CreateJob|2xx->4xx|status
Issue-Key: iot|DescribeJob|2xx->4xx|$.job
Issue-Key: iot|DescribeJob|2xx->4xx|status
Issue-Key: iot|CreateJob|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateJob|4xx->4xx|status
Issue-Key: iot|DeleteJob|2xx->4xx|status
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.isConcurrent
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobExecutionsRolloutConfig
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.numberOfCanceledThings
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.numberOfFailedThings
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.numberOfInProgressThings
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.numberOfQueuedThings
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.numberOfRejectedThings
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.numberOfRemovedThings
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.numberOfSucceededThings
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.numberOfTimedOutThings
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.jobProcessDetails.processingTargets
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.presignedUrlConfig
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.schedulingConfig
Issue-Key: iot|DescribeJob|2xx->2xx|$.job.timeoutConfig
Issue-Key: iot|DeleteCommandExecution|4xx->2xx|$
Issue-Key: iot|DeleteCommandExecution|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteCommandExecution|4xx->2xx|status
Issue-Key: iot|DeleteCommandExecution|4xx->4xx|$.Message
Issue-Key: iot|DeleteCommandExecution|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteCommandExecution|4xx->4xx|status
Issue-Key: iot|GetCommand|2xx->2xx|$.deprecated
Issue-Key: iot|GetCommand|2xx->2xx|$.pendingDeletion
Issue-Related: 20261107T022027Z-iot-already-exists-conflicts-9-causes-reworded-conflictexception.md
Issue-Commit: ff097a89ab5b098f807ae300b709617042ce117b

# iot jobs and commands plane: 8 causes — continuous jobs unreachable end to end and execution refusals answer wrong statuses

> _Note: this issue was identified against the code base at commit `ff097a89ab5b098f807ae300b709617042ce117b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
The emulator cannot serve a continuous IoT job at all: CreateJob refuses a thing-group target outright, so every read, re-create and delete in the same flow answers the wrong status; where a job IS created, DescribeJob's projection omits the defaulted sub-structures AWS always sends and reports target/progress counters the job does not have. The commands plane has the mirror-image problem: the targetArn-ownership refusal answers the wrong status and class, and a command marked for deletion never reports that state. 8 net-new causes, all in service iot.

Cause 1 of 8: CreateJob refuses a thing-group target.
AWS: Smithy: CreateJob's targets is a list of ARNs and the operation's modelled client errors are InvalidRequestException (400), ResourceNotFoundException (404), ResourceAlreadyExistsException (409), LimitExceededException and ThrottlingException; a thing-group ARN is a legal target (Job.targets in the recorded response is the thinggroup ARN). Recorded behavior: 200 with jobId, jobArn and description echoed.
Emulator: 400 with the message 'a thing group target cannot be expanded: this emulator records no membership for thing group <name>, so the devices it names are not known' and x-amzn-errortype 'InvalidRequestException' — none of jobId/jobArn/description returned.
Why: AWS does not require group membership to be resolvable at create time (a CONTINUOUS job targets the group and picks up devices as they join); refusing the create makes the entire continuous-job workflow unusable and the emulator's message is self-referential prose AWS never emits.
Fix: the CreateJob handler's target validation — accept a thinggroup ARN target without expanding membership.
Traces: iot-job-continuous-lifecycle#task#6.

Cause 2 of 8: DescribeJob 404s for the job cause 1 refused.
AWS: Smithy: DescribeJob returns job (Job) with ResourceNotFoundException (404) when absent. Recorded behavior: 200 with job populated — jobId, jobArn, description, status 'IN_PROGRESS', targetSelection 'CONTINUOUS', targets (the thinggroup ARN), isConcurrent true, and the defaulted sub-structures jobExecutionsRolloutConfig, jobProcessDetails (all eight counters at 0, processingTargets null), presignedUrlConfig, schedulingConfig and timeoutConfig.
Emulator: 404, job absent.
Why: the read that confirms cause 1 is cleared; a client polling job status sees not-found for a job it was told nothing about.
Fix: follows from cause 1.
Traces: iot-job-continuous-lifecycle#task#7 (also #task#9, #task#13).

Cause 3 of 8: re-creating the same jobId answers 400 instead of the 409 conflict.
AWS: recorded behavior: 409 with the message 'Job ' + <jobId> + ' already exists.'
Emulator: 400 with cause 1's thing-group-expansion prose and x-amzn-errortype 'InvalidRequestException'.
Why: the conflict is never reachable, so a client that uses CreateJob idempotently (create, catch conflict, continue) takes the validation branch instead and aborts.
Fix: follows from cause 1 — the duplicate-id check must run and win over target expansion.
Traces: iot-job-continuous-lifecycle#task#14.

Cause 4 of 8: DeleteJob 404s where AWS deletes the job.
AWS: recorded behavior: 200.
Emulator: 404.
Why: teardown reports failure for a job the client believes it created; closes the cause-1 cascade.
Fix: follows from cause 1.
Traces: iot-job-continuous-lifecycle#teardown#16.

Cause 5 of 8: DescribeJob's projection omits every defaulted sub-structure and reports phantom targets/counters.
AWS: Smithy: DescribeJob output member job targets Job. Recorded behavior: AWS always emits the job's defaulted sub-structures with their members null rather than omitting them — isConcurrent, jobExecutionsRolloutConfig {exponentialRate, maximumPerMinute}, presignedUrlConfig {expiresInSec, roleArn}, schedulingConfig {endBehavior, endTime, maintenanceWindows, startTime}, timeoutConfig {inProgressTimeoutInMinutes} — and jobProcessDetails with all eight counters present and processingTargets null for a job whose targets have not been expanded.
Emulator: omits isConcurrent, jobExecutionsRolloutConfig, presignedUrlConfig, schedulingConfig, timeoutConfig and seven of the eight jobProcessDetails counters; reports numberOfQueuedThings 1 where AWS reports 0, and populates processingTargets with a thing name where AWS sends null.
Why: a client reading job progress gets absent fields where AWS guarantees a structure (so counter arithmetic over the response throws or silently treats absent as zero) and is told one device is queued when AWS reports none — it will wait for an execution that does not exist.
Fix: the DescribeJob serializer must emit the defaulted sub-structures, and the job state must not pre-populate processingTargets/numberOfQueuedThings before executions are created.
Traces: iot-job-list-lifecycle#task#4.

Cause 6 of 8: DeleteCommandExecution accepts a targetArn the caller does not own.
AWS: Smithy: DeleteCommandExecution.targetArn is @required httpQuery; the operation's modelled client errors are ValidationException (400) and ConflictException (409) — AWS's recorded refusal for an unowned target is NOT one of these, so recorded behavior only: 403 with x-amzn-errortype 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/' and the body member 'Message' (capital M) set to the fixed text 'targetArn not found in account' — a constant, with no variable part.
Emulator: 200 with an empty body and no errortype header — it performs the delete.
Why: this is an authorization boundary: the emulator lets a caller delete a command execution against an ARN outside its account, and gives no signal at all.
Fix: the DeleteCommandExecution handler's target-ownership check, and the error serializer must support the capital-M 'Message' member and the coral-namespace errortype AWS uses for this refusal.
Traces: iot-command-duplicate-and-execution-errors#task#4.

Cause 7 of 8: the same unowned-target refusal, reached through a different request shape, answers 400 ValidationException.
AWS: recorded behavior: 403, errortype 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', body member 'Message' = 'targetArn not found in account'.
Emulator: 400 with x-amzn-errortype 'ValidationException' and no 'Message' member.
Why: distinguishes the two failure modes — here the emulator does refuse, but as a request-validation error, so a caller cannot tell a permissions problem from a malformed request and will retry with a 'fixed' ARN forever.
Fix: as cause 6 — classify the unowned target as the access-denied refusal, not a constraint violation.
Traces: iot-command-duplicate-and-execution-errors#task#5.

Cause 8 of 8: GetCommand never reports deprecated/pendingDeletion.
AWS: Smithy: GetCommand output carries deprecated (DeprecationFlag, boolean) and pendingDeletion (BooleanWrapperObject, boolean). Recorded behavior: after the command has been deprecated and marked for deletion, GetCommand reports deprecated true and pendingDeletion true.
Emulator: reports both as false.
Why: these two flags are how a client learns a command is being retired; always-false means a caller keeps issuing a command AWS has scheduled for removal and never observes the retirement it requested.
Fix: the command state's deprecation/deletion flags and the GetCommand serializer must read them.
Traces: iot-command-duplicate-and-execution-errors#teardown#7.

Related open issues that do NOT cover these: '20261106T201842Z-iot-jobs-plane-8-net-new-causes-createjob.md' is a behavioral finding and '20261106T201842Z-iot-deletecommand-stub.md' / '20261106T201842Z-iot-canceljob-stub.md' concern other operations; none carries a replay key for the eight fixture steps above, and the replay reports no stub blocking them.

## Evidence
Replay of trace set simparity-wire-aws-v1. Expected = recorded AWS response, Actual = emulator.

1. CreateJob, iot-job-continuous-lifecycle#task#6 — status expected 200, got 400; $.description expected 'probe job', $.jobArn and $.jobId expected present, all got absent. (The emulator's 400 message is shown in cluster 3.)
2. DescribeJob, iot-job-continuous-lifecycle#task#7 — status expected 200, got 404; $.job expected populated with status 'IN_PROGRESS', targetSelection 'CONTINUOUS', targets ['arn:aws:iot:us-west-2:000000000000:thinggroup/sim2real-probe-iot-thing-group-e411e088'], isConcurrent true, jobProcessDetails all-zero counters, and the defaulted jobExecutionsRolloutConfig/presignedUrlConfig/schedulingConfig/timeoutConfig structures; got absent. Cluster of 3.
3. CreateJob, iot-job-continuous-lifecycle#task#14 — status expected 409, got 400; $.message expected 'Job sim2real-probe-iot-job-e411e088 already exists.', got 'a thing group target cannot be expanded: this emulator records no membership for thing group sim2real-probe-iot-thing-group-e411e088, so the devices it names are not known'; x-amzn-errortype got 'InvalidRequestException'.
4. DeleteJob, iot-job-continuous-lifecycle#teardown#16 — status expected 200, got 404.
5. DescribeJob, iot-job-list-lifecycle#task#4 — status 200 both; $.job.isConcurrent expected true, got absent; $.job.jobExecutionsRolloutConfig, $.job.presignedUrlConfig, $.job.schedulingConfig, $.job.timeoutConfig expected present, got absent; $.job.jobProcessDetails.numberOfCanceledThings/FailedThings/InProgressThings/RejectedThings/RemovedThings/SucceededThings/TimedOutThings expected 0, got absent; numberOfQueuedThings expected 0, got 1; processingTargets expected null, got ['sim2real-probe-iot-job-thing-5dd167eb'].
6. DeleteCommandExecution, iot-command-duplicate-and-execution-errors#task#4 — status expected 403, got 200; body expected {'Message': 'targetArn not found in account'}, got {}; x-amzn-errortype expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', got absent.
7. DeleteCommandExecution, iot-command-duplicate-and-execution-errors#task#5 — status expected 403, got 400; $.Message expected 'targetArn not found in account', got absent; x-amzn-errortype expected 'AccessDeniedException:http://internal.amazon.com/coral/com.amazon.coral.service/', got 'ValidationException'.
8. GetCommand, iot-command-duplicate-and-execution-errors#teardown#7 — status 200 both; $.deprecated expected true, got false; $.pendingDeletion expected true, got false.

(Job ids, group names, ARNs and thing names above are this recording's values; the requirements are the statuses, the fixed message texts, the always-present defaulted sub-structures and the flag semantics described per cause.)

## Steps to Reproduce
1. git clone the repo and boot the emulator from this checkout.
2. Replay the simparity-wire-aws-v1 wire corpus.
3. Observe the eight clusters listed under Evidence, starting with fixture iot-job-continuous-lifecycle step task#6.
4. Equivalently by hand: create a thing group, then POST /jobs/<j> with targets=[that group's ARN] and targetSelection CONTINUOUS (expect 200); GET /jobs/<j> (expect 200 with the defaulted sub-structures and all-zero jobProcessDetails); POST /jobs/<j> again (expect 409 'Job <j> already exists.'); DELETE /jobs/<j> (expect 200). For commands: PUT /commands/<c>, then DELETE /command-executions/<e>?targetArn=<an ARN in another account> (expect 403 with Message 'targetArn not found in account'); deprecate and delete the command, then GET /commands/<c> and check deprecated/pendingDeletion are true.
