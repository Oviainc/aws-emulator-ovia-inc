Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: mediaconvert.concurrent-create-queue-same-name-race mediaconvert.create-job-unassumable-role-forbidden mediaconvert.delete-default-queue-rejected mediaconvert.update-queue-invalid-status-rejected
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|CreateQueue|ConflictException
Issue-Key: mediaconvert.create-job-unassumable-role-forbidden|_error#12|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: mediaconvert.delete-default-queue-rejected|run#3|AssertionError|assert code == "ForbiddenException", f"expected ForbiddenException, got {code!r}"
Issue-Key: mediaconvert.update-queue-invalid-status-rejected|run#8|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# MediaConvert queue/job control plane: 4 fidelity divergences (CreateQueue not idempotent, CreateJob accepts unassumable Role, Default SYSTEM queue unprotected, UpdateQueue enum message)

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateQueue

## Description
This issue bundles 4 distinct root causes in the MediaConvert emulator's queue and job control plane. All 4 must be closed before submitting.

=== Cause 1 of 4: CreateQueue is not idempotent on Name; a repeat/concurrent create with the same Name is rejected with ConflictException ===
AWS: recorded behavior only (the Smithy model constrains neither way: CreateQueue input Name is @required __string, and ConflictException is merely one of the modeled errors - BadRequestException 400, ConflictException 409, ForbiddenException 403, NotFoundException 404, ServiceQuotaExceededException 402, TooManyRequestsException 429, InternalServerErrorException 500; http POST /2017-08-29/queues -> 201). The recording (replayed twice against real AWS, per the check's own docstring) shows CreateQueue is idempotent on Name: N concurrent racers calling CreateQueue with one identical Name ALL succeed, every caller gets back the SAME Queue.Arn, and exactly one queue exists afterwards (confirmed by GetQueue and by ListQueues returning exactly one entry with that Name).
Emulator: exactly one racer wins and the losers are rejected - ConflictException (409) 'Queue <name> already exists.' propagates out of CreateQueue, so the check errors.
Why: a client that treats CreateQueue as idempotent (retry-safe provisioning, concurrent bootstrap of the same queue from several workers, SDK retries after a timed-out-but-applied create) sees a hard 409 where AWS returns success and the existing queue. Idempotent setup code becomes non-deterministic and must grow conflict-swallowing it does not need on AWS.
Fix: the CreateQueue handler and its queue state - on an existing queue of the same Name, return the existing queue (200/201 with the same ARN) instead of raising ConflictException, and make the existence-check/insert atomic so concurrent racers cannot both be penalized.
Traces: mediaconvert.concurrent-create-queue-same-name-race.

=== Cause 2 of 4: CreateJob accepts a Role that does not trust mediaconvert.amazonaws.com instead of rejecting it with ForbiddenException ===
AWS: Smithy: CreateJob.Role is @required __string (jsonName 'role'); CreateJob models ForbiddenException with httpError 403. Recorded behavior: when the supplied Role's trust policy does not allow sts:AssumeRole for the mediaconvert.amazonaws.com service principal, CreateJob is rejected synchronously with error code ForbiddenException (HTTP 403) and no job is created (ListJobs on the queue stays empty). The recorded message is an example of a request-derived template: 'The service does not have permission to assume the IAM role: '<roleArn>'. Please verify the role exists and has granted MediaConvert the sts:AssumeRole permission, then resubmit your job. For more information about IAM role setup, see: https://docs.aws.amazon.com/mediaconvert/latest/ug/iam-role.html' - the rule is that the message names the offending role ARN; the exact prose is an example, the 403/ForbiddenException pair is the requirement.
Emulator: the call SUCCEEDS - HTTP 201 with a full Job body (Status 'SUBMITTED', a generated Id/Arn, and the untrusted Role echoed back). The emulator performs no trust-policy check on the Role at all. The check's positive control (a role that DOES trust mediaconvert.amazonaws.com) is the behavior the emulator applies to every role.
Why: this is an over-permissive acceptance. A client validating its IAM wiring by submitting a job gets a green 201 and a SUBMITTED job where AWS refuses the request, so a misconfigured trust policy is never surfaced; negative-path tests and guardrails that assert ForbiddenException pass against the emulator and fail in production. It also leaks state AWS never creates (a job on the queue).
Fix: CreateJob's validation - resolve the Role ARN against IAM state, inspect its assume-role trust policy for a Statement allowing sts:AssumeRole to Principal.Service mediaconvert.amazonaws.com, and raise ForbiddenException (403) naming the role ARN before any job is persisted.
Traces: mediaconvert.create-job-unassumable-role-forbidden.

=== Cause 3 of 4: the account's built-in 'Default' SYSTEM queue is not modeled, so DeleteQueue on it is not rejected (and DeleteQueue answers NotImplemented) ===
AWS: Smithy: DeleteQueue takes Name as an @httpLabel @required member on DELETE /2017-08-29/queues/{Name} (success 202) and models ForbiddenException httpError 403 and NotFoundException httpError 404. Recorded behavior: every account has a built-in queue named 'Default' whose Queue.Type is 'SYSTEM'; DeleteQueue on it is rejected with ForbiddenException (403) and the queue still exists afterwards (GetQueue(Name='Default') returns Name 'Default', Type 'SYSTEM'). A user-created CUSTOM queue deletes cleanly, after which both GetQueue and UpdateQueue on that name fail with NotFoundException (404). Recorded messages, as examples of request-derived templates: 'You specified an invalid value for name. You are not allowed to delete system resources.' and 'The resource you specified in name was not found.'
Emulator: DeleteQueue(Name='Default') returns error code 'NotImplemented' (the operation is a scaffold stub in services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/delete_queue.rs), so neither the SYSTEM-resource rejection nor the post-delete NotFoundException behavior exists. NOTE: a separate already-filed issue covers DeleteQueue merely being an unimplemented stub (it was attributed to mediaconvert.queue-create-get-delete-lifecycle). That issue does NOT cover this cause: beyond implementing the operation, the emulator must seed the built-in 'Default' queue with Type SYSTEM and make DeleteQueue refuse SYSTEM-typed queues with ForbiddenException.
Why: a client that relies on 'Default' always being present (job submission without an explicit Queue, console-equivalent listings) finds no such queue, and teardown code that deletes every listed queue would wipe a resource AWS protects. Code asserting ForbiddenException for system resources instead sees NotImplemented.
Fix: MediaConvert queue state (seed a 'Default' queue with Type SYSTEM at account initialization) plus the DeleteQueue handler (reject Type SYSTEM with ForbiddenException 403; 404 NotFoundException for unknown names, including after a successful delete, consistently with GetQueue/UpdateQueue).
Traces: mediaconvert.delete-default-queue-rejected.

=== Cause 4 of 4: UpdateQueue's invalid-enum rejection uses a generic constraint message instead of MediaConvert's member/shape-derived message ===
AWS: Smithy: UpdateQueue.Status targets the QueueStatus enum (jsonName 'status'); UpdateQueue models BadRequestException httpError 400. The error CODE and status match already (BadRequestException / 400) - only the message diverges. The message rule is known from the recording and is derived from the request and the model: it names the rejected member in its wire form ('status'), the target shape name (QueueStatus) and that shape's value set, e.g. "You specified an invalid type for 'status'. Replace the value with a QueueStatus (Valid values: 'ACTIVE', 'PAUSED'), or remove 'status' (if optional), then resubmit your request. For more information, see the MediaConvert API reference: https://docs.aws.amazon.com/mediaconvert/latest/apireference/resources.html". Hypothesis: this template is MediaConvert-wide for any enum-typed member, not specific to Status.
Emulator: returns BadRequestException with the generic cross-service constraint text "Value 'BOGUS_STATUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, PAUSED]".
Why: clients and test suites that match on MediaConvert's documented message shape (or surface it to operators) see a foreign message; the service-specific remediation guidance AWS returns is lost.
Fix: MediaConvert's input validation / error serializer - emit the MediaConvert enum-rejection template parameterized by the member's jsonName, its target enum shape name and that shape's values, instead of the generic framework constraint message. The remainder of the check (code BadRequestException, Status left unchanged at ACTIVE, and a valid PAUSED update persisting) is not in dispute.
Traces: mediaconvert.update-queue-invalid-status-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1, mediaconvert 0/21 (0%).

Cause 1 - mediaconvert.concurrent-create-queue-same-name-race [error]
  Expected: 8 concurrent CreateQueue calls with one identical Name all succeed and return the same Queue.Arn; GetQueue resolves it; ListQueues shows exactly one queue with that Name.
  Actual: botocore.errorfactory.ConflictException: An error occurred (ConflictException) when calling the CreateQueue operation: Queue sim2real-mc-queue-race-<id> already exists.

Cause 2 - mediaconvert.create-job-unassumable-role-forbidden [mismatch]
  Expected: CreateJob(Role=<role trusting only ec2.amazonaws.com>) -> ClientError code 'ForbiddenException' (HTTP 403); ListJobs(Queue=...) == [].
  Actual: AssertionError: expected the call to be rejected, but it SUCCEEDED and returned HTTPStatusCode 201 with Job{ Id '<generated>', Arn 'arn:aws:mediaconvert:<region>:<account>:jobs/<generated>', Role '<the untrusted role arn>', Status 'SUBMITTED' }.

Cause 3 - mediaconvert.delete-default-queue-rejected [mismatch]
  Expected: delete_queue(Name='Default') -> code 'ForbiddenException' (403); get_queue(Name='Default') -> Name 'Default', Type 'SYSTEM'.
  Actual: AssertionError: expected ForbiddenException, got 'NotImplemented' (DeleteQueue is a scaffold stub: services/service-com-amazonaws-mediaconvert-mediaconvert/src/ops/delete_queue.rs). The downstream assertions (deleted CUSTOM queue reading NotFoundException 404 from GetQueue and UpdateQueue) were never reached.

Cause 4 - mediaconvert.update-queue-invalid-status-rejected [mismatch]
  Code/status agree (BadRequestException, 400); message diverges.
  Expected (rule: template naming the member jsonName, the target enum shape and its values): "You specified an invalid type for 'status'. Replace the value with a QueueStatus (Valid values: 'ACTIVE', 'PAUSED'), or remove 'status' (if optional), then resubmit your request. For more information, see the MediaConvert API reference: https://docs.aws.amazon.com/mediaconvert/latest/apireference/resources.html"
  Actual: "Value 'BOGUS_STATUS' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [ACTIVE, PAUSED]"

Model citations used: Smithy CreateQueue (POST /2017-08-29/queues, 201; errors BadRequestException 400 / ConflictException 409 / ForbiddenException 403 / NotFoundException 404 / ServiceQuotaExceededException 402 / TooManyRequestsException 429 / InternalServerErrorException 500); DeleteQueue (DELETE /2017-08-29/queues/{Name}, 202, same error set); UpdateQueue (PUT /2017-08-29/queues/{Name}, 200, Status -> QueueStatus enum); CreateJob (POST /2017-08-29/jobs, 201, Role @required, ForbiddenException 403).

## Steps to Reproduce
Boot the emulator and point a botocore mediaconvert client at it (region us-west-2).

Cause 1: pick one queue name N. From 8 threads call create_queue(Name=N) concurrently (or simply call create_queue(Name=N) twice sequentially). Expect every call to succeed with the same Queue.Arn; observe ConflictException 'Queue N already exists.' on all but the first. Then get_queue(Name=N) / list_queues() should show exactly one queue.

Cause 2: iam.create_role with a trust policy allowing sts:AssumeRole only to Principal.Service 'ec2.amazonaws.com'; create_queue(Name=Q, Status='PAUSED'); call create_job(Role=<that role arn>, Queue=Q, Settings={'Inputs':[{'FileInput':'s3://b/in.mp4'}],'OutputGroups':[{...FILE_GROUP_SETTINGS -> s3://b/out/ , one MP4/H_264 output...}]}). Expect ClientError ForbiddenException (403) and list_jobs(Queue=Q)['Jobs'] == []; observe HTTP 201 and a SUBMITTED job.

Cause 3: call delete_queue(Name='Default'). Expect ForbiddenException (403); observe NotImplemented. Then get_queue(Name='Default') should return Type 'SYSTEM'. Positive control: create_queue(Name=N2); delete_queue(Name=N2) should succeed, after which get_queue(Name=N2) and update_queue(Name=N2, Status='PAUSED') must both raise NotFoundException (404).

Cause 4: create_queue(Name=N3); call update_queue(Name=N3, Status='BOGUS_STATUS'). Error code is BadRequestException as expected; compare the Message against the MediaConvert template naming 'status' / QueueStatus / its valid values. Then get_queue(Name=N3)['Queue']['Status'] must still be 'ACTIVE', and update_queue(Name=N3, Status='PAUSED') must persist.
