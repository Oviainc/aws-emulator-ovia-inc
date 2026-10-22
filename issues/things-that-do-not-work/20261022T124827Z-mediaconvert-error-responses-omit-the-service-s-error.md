Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconvert-job-submission-workflow
Issue-Step: task#11
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconvert-job-submission-workflow mediaconvert-job-template-lifecycle mediaconvert-queue-lifecycle
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|GetJob|4xx->4xx|$.accountId
Issue-Key: mediaconvert|GetJob|4xx->4xx|$.errorType
Issue-Key: mediaconvert|GetJob|4xx->4xx|$.httpStatus
Issue-Key: mediaconvert|GetJob|4xx->4xx|$.operation
Issue-Key: mediaconvert|GetJob|4xx->4xx|$.requestId
Issue-Key: mediaconvert|GetJob|4xx->4xx|$.settingsValidationErrorsJsonBlob
Issue-Key: mediaconvert|GetJobTemplate|4xx->4xx|$.accountId
Issue-Key: mediaconvert|GetJobTemplate|4xx->4xx|$.errorType
Issue-Key: mediaconvert|GetJobTemplate|4xx->4xx|$.httpStatus
Issue-Key: mediaconvert|GetJobTemplate|4xx->4xx|$.operation
Issue-Key: mediaconvert|GetJobTemplate|4xx->4xx|$.requestId
Issue-Key: mediaconvert|GetJobTemplate|4xx->4xx|$.settingsValidationErrorsJsonBlob
Issue-Key: mediaconvert|GetQueue|4xx->4xx|$.accountId
Issue-Key: mediaconvert|GetQueue|4xx->4xx|$.errorType
Issue-Key: mediaconvert|GetQueue|4xx->4xx|$.httpStatus
Issue-Key: mediaconvert|GetQueue|4xx->4xx|$.operation
Issue-Key: mediaconvert|GetQueue|4xx->4xx|$.requestId
Issue-Key: mediaconvert|GetQueue|4xx->4xx|$.settingsValidationErrorsJsonBlob
Issue-Commit: 9979b14c0e509985792db6dac0171a1eff5d6a10

# MediaConvert error responses omit the service's error envelope members

> _Note: this issue was identified against the code base at commit `9979b14c0e509985792db6dac0171a1eff5d6a10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJob

## Description
This issue bundles 3 distinct net-new WIRE root causes with one shared fault: when the MediaConvert emulator returns a 404 NotFoundException the STATUS is right but the error BODY carries only a message, while AWS's MediaConvert error envelope carries errorType, httpStatus, accountId, requestId, operation and settingsValidationErrorsJsonBlob. All 3 must be closed before submitting. No open issue covers them (the two open mediaconvert issues are behavioral and concern queue/job semantics, not the error envelope), and the replay reports owned_by=[] for all three clusters.

=== Cause 1 of 3: GetJob's 404 body omits the MediaConvert error envelope ===
AWS: Smithy: GetJob is GET /2017-08-29/jobs/{Id} -> 200, with NotFoundException modeled at httpError 404; the modeled NotFoundException shape declares only Message (jsonName 'message'), so the rest is recorded behavior only. Recorded behavior: the 404 body is MediaConvert's REST error envelope - errorType 'NotFoundException', httpStatus 404 (the status repeated in the body), accountId (the caller's account), requestId (a per-request id), operation 'GetJob' (the operation that failed) and settingsValidationErrorsJsonBlob '' (empty when the failure is not a settings-validation failure) - alongside message. The envelope VALUES follow request-derived rules: errorType is the error's code, httpStatus equals the HTTP status, operation is the operation name, settingsValidationErrorsJsonBlob is empty for a non-validation error; accountId and requestId are environment-specific and only their presence is a requirement.
Emulator: status 404 is correct, but every one of those six members is absent from the body.
Why: clients and operators read these members. Anything that classifies failures from the body (rather than the header) - log pipelines, error dashboards, SDK-independent HTTP clients, retry logic keyed on errorType/httpStatus - sees nothing, and correlating a failure to a request through requestId or to an operation through operation is impossible. Tooling that reads response['Error'] plus the envelope gets a shape AWS never returns.
Fix: the service's error serializer (shared by every operation's error path), so a modeled MediaConvert exception renders the full envelope - errorType from the error's code, httpStatus from the HTTP status, operation from the operation being served, accountId from the request's resolved account, requestId from the per-request id, and settingsValidationErrorsJsonBlob defaulted to '' - rather than message alone.
Traces: mediaconvert-job-submission-workflow#task#11.

=== Cause 2 of 3: GetJobTemplate's 404 body omits the same envelope ===
AWS: Smithy: GetJobTemplate is GET /2017-08-29/jobTemplates/{Name} -> 200 with NotFoundException httpError 404. Recorded behavior: the 404 after the template is deleted carries errorType 'NotFoundException', httpStatus 404, accountId, requestId, operation 'GetJobTemplate' and settingsValidationErrorsJsonBlob ''.
Emulator: status 404 correct, all six members absent.
Why: as cause 1; note the operation member is per-operation, so fixing one operation's path does not fix this one - the envelope must be produced for every operation.
Fix: the same shared error serializer, with operation taken from the operation being served (here GetJobTemplate).
Traces: mediaconvert-job-template-lifecycle#teardown#8.

=== Cause 3 of 3: GetQueue's 404 body omits the same envelope ===
AWS: Smithy: GetQueue is GET /2017-08-29/queues/{Name} -> 200 with NotFoundException httpError 404. Recorded behavior: the 404 after the queue is deleted carries errorType 'NotFoundException', httpStatus 404, accountId, requestId, operation 'GetQueue' and settingsValidationErrorsJsonBlob ''.
Emulator: status 404 correct, all six members absent.
Why: as cause 1, on the queue plane - and this is the read a client uses to confirm a delete, so the envelope it inspects is wrong exactly where it matters.
Fix: the same shared error serializer, with operation = GetQueue.
Traces: mediaconvert-queue-lifecycle#teardown#13.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator: 62 steps, 27 passed, score 43.5%. bootstrap_signal=false, emulator_up=true, state_reset_failures=0. In all three clusters expected_status == actual_status == 404; only the body diverges, so this is a serializer defect and not a status or routing defect.

Cause 1 - mediaconvert-job-submission-workflow#task#11, GetJob, status 404 == 404 [mismatch]
  Expected body members: errorType 'NotFoundException'; httpStatus 404; accountId '<caller account>'; requestId <present, value not asserted>; operation 'GetJob'; settingsValidationErrorsJsonBlob ''
  Actual:   all six missing (None).

Cause 2 - mediaconvert-job-template-lifecycle#teardown#8, GetJobTemplate, status 404 == 404 [mismatch]
  Expected body members: errorType 'NotFoundException'; httpStatus 404; accountId '<caller account>'; requestId <present>; operation 'GetJobTemplate'; settingsValidationErrorsJsonBlob ''
  Actual:   all six missing (None).

Cause 3 - mediaconvert-queue-lifecycle#teardown#13, GetQueue, status 404 == 404 [mismatch]
  Expected body members: errorType 'NotFoundException'; httpStatus 404; accountId '<caller account>'; requestId <present>; operation 'GetQueue'; settingsValidationErrorsJsonBlob ''
  Actual:   all six missing (None).

Note on what is NOT claimed: the recorded accountId and requestId values are environment-specific (the fixture's requestId is recorded as shaped-not-asserted), so only their PRESENCE is treated as the requirement, not their values. The error message text is not in dispute in any of these three clusters.

Model citations used: Smithy GetJob (GET /2017-08-29/jobs/{Id}, 200), GetJobTemplate (GET /2017-08-29/jobTemplates/{Name}, 200), GetQueue (GET /2017-08-29/queues/{Name}, 200), each modeling NotFoundException at httpError 404; NotFoundException (structure, errorKind client, httpError 404, members: Message jsonName 'message' only - the envelope members are recorded behavior only).

## Steps to Reproduce
Boot the emulator and point an HTTP client (or botocore with the raw response captured) at it, region us-west-2.

Cause 1: GET /2017-08-29/jobs/<an-id-that-does-not-exist>. Expect HTTP 404 whose JSON body carries errorType 'NotFoundException', httpStatus 404, accountId, requestId, operation 'GetJob' and settingsValidationErrorsJsonBlob ''; observe a 404 whose body has none of them.

Cause 2: create_job_template(Name=T, Settings={...}); delete_job_template(Name=T); GET /2017-08-29/jobTemplates/T. Expect the same envelope with operation 'GetJobTemplate'; observe the members absent.

Cause 3: create_queue(Name=Q); delete_queue(Name=Q); GET /2017-08-29/queues/Q. Expect the same envelope with operation 'GetQueue'; observe the members absent.

In each case compare the full decoded body, not just the status or the SDK's Error.Code - the divergence is only visible in the body members.
