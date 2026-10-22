Issue-Source: trace-sweep-divergence
Issue-Trace: mediaconvert-jobs-query-workflow
Issue-Step: task#3
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mediaconvert-jobs-query-workflow
Issue-Shape-Id: com.amazonaws.mediaconvert#MediaConvert
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.accountId
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.errorType
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.httpStatus
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.operation
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.requestId
Issue-Key: mediaconvert|GetJobsQueryResults|4xx->4xx|$.settingsValidationErrorsJsonBlob
Issue-Commit: b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3

# MediaConvert GetJobsQueryResults 404 omits the error envelope members

> _Note: this issue was identified against the code base at commit `b4ac6d961cdb10c212ccdfac1172ee263c3e4aa3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetJobsQueryResults

## Description
This issue carries 1 net-new WIRE root cause. It must be closed before submitting.

=== Cause 1 of 1: GetJobsQueryResults' 404 NotFoundException body omits MediaConvert's error envelope ===
AWS: Smithy (com.amazonaws.mediaconvert#MediaConvert): GetJobsQueryResults is GET /2017-08-29/jobsQueries/{Id} -> 200, with NotFoundException modeled at httpError 404 (errorKind client); the modeled NotFoundException structure declares only Message (jsonName 'message'), so the remaining envelope members are recorded behavior only. Recorded behavior: the 404 response body is MediaConvert's REST error envelope - errorType ('NotFoundException', i.e. the error's code), httpStatus (404, the HTTP status repeated inside the body), accountId (the caller's resolved account), requestId (the per-request id), operation ('GetJobsQueryResults', the operation that failed) and settingsValidationErrorsJsonBlob ('' when the failure is not a settings-validation failure) - alongside message. Those values follow request-derived rules, so the rules are the requirement; accountId and requestId are environment-specific and only their PRESENCE is a requirement (the fixture records requestId as shaped-not-asserted).
Emulator: the HTTP status is correct (expected 404, actual 404), but every one of those six envelope members is absent from the body - the emulator emits a message-only body.
Why: a client that classifies failures from the response body rather than the x-amzn-errortype header - log/metric pipelines, error dashboards, non-SDK HTTP clients, retry logic keyed on errorType or httpStatus - sees nothing to key on, and a failure cannot be correlated back to a request through requestId or to an operation through operation. Tooling written against AWS's MediaConvert envelope gets a body shape AWS never returns.
Fix: the service's shared error serializer on the operation's error path, so a modeled MediaConvert exception renders the full envelope (errorType from the error's code, httpStatus from the HTTP status, operation from the operation being served, accountId from the request's resolved account, requestId from the per-request id, settingsValidationErrorsJsonBlob defaulted to '') instead of message alone. Note that the operation member is per-operation, so a fix applied to one operation's handler does not cover this one; the envelope must be produced for every operation, GetJobsQueryResults included.
Traces: mediaconvert-jobs-query-workflow#task#3.

Relation to open issues: '20261022T124827Z-mediaconvert-error-responses-omit-the-service-s-error.md' describes the same shared fault for GetJob, GetJobTemplate and GetQueue, but it does not carry this cluster's replay key (the replay reports owned_by=[] for GetJobsQueryResults), so this operation's 404 body is not gated by it and is filed here. A fix to the shared serializer will likely close both, but this cluster must be re-replayed on its own.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator: 62 steps total, 29 passed, 33 mismatched, score 46.8%. bootstrap_signal=false, emulator_up=true, state_reset_failures=0, so the result is trustworthy and the emulator is serving mediaconvert (many steps of the same fixtures pass).

Cause 1 - fixture mediaconvert-jobs-query-workflow, step task#3, operation GetJobsQueryResults, verdict mismatch, expected_status 404, actual_status 404, owned_by=[], stub_path="".
  Expected body members: errorType 'NotFoundException'; httpStatus 404; accountId '000000000000' (the caller's account in this recording); requestId <present, value not asserted>; operation 'GetJobsQueryResults'; settingsValidationErrorsJsonBlob ''.
  Actual: all six members missing (None).

Because expected_status == actual_status == 404, this is purely an error-body serialization defect, not a status, routing or state defect.

Not claimed as requirements: the literal accountId and requestId values (environment-specific - only presence is required), and the error message text, which is not in dispute in this cluster.

Model citations used: smithy_query operation=GetJobsQueryResults on services/service-com-amazonaws-mediaconvert-mediaconvert/smithy-model/com-amazonaws-mediaconvert-mediaconvert.json -> http GET /2017-08-29/jobsQueries/{Id} code 200, input Id (httpLabel, required), errors include NotFoundException httpError 404; NotFoundException declares Message (jsonName 'message') only.

## Steps to Reproduce
Boot the emulator and point an HTTP client (or botocore with the raw HTTP response captured) at it, region us-west-2.

Cause 1: issue GET /2017-08-29/jobsQueries/<a-jobs-query-id-that-does-not-exist> (the fixture reaches this state by querying the results of a jobs query id that is not present). Expect HTTP 404 whose decoded JSON body carries errorType 'NotFoundException', httpStatus 404, accountId, requestId, operation 'GetJobsQueryResults' and settingsValidationErrorsJsonBlob '' in addition to message. Observe HTTP 404 whose body contains none of those six members.

Compare the FULL decoded response body, not just the status code or the SDK's Error.Code - the divergence is only visible in the body members. Re-replay fixture mediaconvert-jobs-query-workflow step task#3 to confirm the fix.
