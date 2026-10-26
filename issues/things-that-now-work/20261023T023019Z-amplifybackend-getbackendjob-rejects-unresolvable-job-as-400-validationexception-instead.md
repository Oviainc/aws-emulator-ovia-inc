Issue-Source: trace-sweep-divergence
Issue-Trace: amplifybackend-get-backend-job-nonexistent-environment
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplifybackend-get-backend-job-nonexistent-environment
Issue-Shape-Id: com.amazonaws.amplifybackend#AmplifyBackend
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|$.code
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|$.message
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|status
Issue-Commit: 1b15cfcf83a9fafcaa69f938ed17066b612a8ba0

# GetBackendJob rejects unresolvable job as 400 ValidationException instead of 404 ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `1b15cfcf83a9fafcaa69f938ed17066b612a8ba0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackendJob

## Description
One root cause, 1 wire divergence cluster (1 fixture step). AmplifyBackend's GetBackendJob read path classifies an (app, environment, job) triple that does not resolve as a malformed-request failure instead of a resource-absent failure, so both the HTTP status class and the error code are wrong.

NOTE ON AN EXISTING ISSUE: the open issue 20261022T142056Z-amplifybackend-createbackend-createbackendauth-getbackendjob-error-contract-wrong-status.md discusses GetBackendJob as its Cause 3, but its wire replay key (Issue-Trace/Issue-Step) binds only amplifybackend-errors-create-backend-app-not-found#task#1 and its Issue-Trace-Ids list carries bare fixture ids with no step half, so the replay reported owned_by=[] for this cluster: this fixture step is NOT covered by that issue and a fix validated against it alone will never be exercised here. The emulator's behavior has also drifted away from what that issue recorded (it recorded 404 with a missing code and an app-lookup message; the emulator now answers 400 ValidationException 'Invalid appId passed.'), so the status class is now wrong as well.

=== Cause 1 of 1: GetBackendJob for an unresolvable job answers 400 ValidationException instead of 404 ResourceNotFoundException ===
AWS: Smithy models GetBackendJob as GET /backend/{AppId}/job/{BackendEnvironmentName}/{JobId} with AppId, BackendEnvironmentName and JobId as required bare __string httpLabels (no documented pattern or length constraint on AppId), binding only BadRequestException(400), GatewayTimeoutException(504), NotFoundException(404) and TooManyRequestsException(429); NotFoundException models Message (jsonName 'message') + ResourceType and BadRequestException models Message only, so neither shape has a `code` member. The error code is recorded behavior only: the recording shows real AmplifyBackend answering an unresolvable (app, environment, job) triple with HTTP 404 and error code `ResourceNotFoundException`, with a message of the form 'An error occurred while processing your request: Unable to find matching job with provided parameters.' AWS does not distinguish 'this app is unknown' from 'this environment/job does not exist' on this path: a well-formed appId that resolves to nothing is a NOT-FOUND, not a request-validation failure.
Emulator: answers HTTP 400 with $.code 'ValidationException' and $.message 'An error occurred while processing your request: Invalid appId passed.' — it treats an unregistered-but-well-formed appId as a malformed input and never reaches a job lookup, so no 404/ResourceNotFoundException is ever produced for this operation.
Why: a client branching on the error classifies a missing job as a permanent, un-retryable bad request and reports the caller's appId as malformed, when the correct reading is 'this job does not exist (yet)'. Pollers that wait for a backend job to appear abort on the 400 instead of continuing, and 400-vs-404 is also the boundary most SDK retry/error-mapping layers key on, so the failure is mis-surfaced as a caller bug rather than absent state.
Fix: GetBackendJob's handler and the app-resolution helper it calls (the amplifybackend service crate's get_backend_job operation handler plus the appId resolution/validation in its state module) — stop rejecting an unresolvable appId as a validation error on this read path and emit a single job-not-found refusal: HTTP 404 with error code ResourceNotFoundException and the service's prefixed 'Unable to find matching job with provided parameters.' message, collapsing the app-missing and job-missing branches into one.
Traces: amplifybackend-get-backend-job-nonexistent-environment#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (summary: 33 steps total, 10 passed, 23 mismatched, 0 errored, 0 blocked, score 30.3%, state_reset_failures 0, bootstrap_signal false, 3 divergence clusters). This cluster reported stub_path="" and owned_by=[] (affected_count 1).

Cause 1 — fixture amplifybackend-get-backend-job-nonexistent-environment, step task#2, operation GetBackendJob, verdict mismatch:
  expected: HTTP 404, $.code 'ResourceNotFoundException', $.message 'An error occurred while processing your request: Unable to find matching job with provided parameters.'
  actual:   HTTP 400, $.code 'ValidationException', $.message 'An error occurred while processing your request: Invalid appId passed.'
  divergence_key: service=amplifybackend|GetBackendJob|status=404->400|keys=$.code,$.message,status

Requirements asserted here are the status (404) and the error code (ResourceNotFoundException). The message is quoted as an example of the service's prefixed not-found text; the status and code are the contract.

Model citations (services/service-com-amazonaws-amplifybackend-amplifybackend/smithy-model/com-amazonaws-amplifybackend-amplifybackend.json): GetBackendJob GET /backend/{AppId}/job/{BackendEnvironmentName}/{JobId}, success code 200, errors BadRequestException(400)/GatewayTimeoutException(504)/NotFoundException(404)/TooManyRequestsException(429); AppId/BackendEnvironmentName/JobId required __string httpLabel with no pattern or length constraint; NotFoundException members Message(jsonName 'message') + ResourceType; BadRequestException member Message only. Neither ValidationException nor ResourceNotFoundException is a modeled shape and no error shape has a `code` member, so the wire error code is recorded behavior only.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture amplifybackend-get-backend-job-nonexistent-environment.
2. Step task#2 issues GET /backend/{appId}/job/{backendEnvironmentName}/{jobId} (GetBackendJob) where the appId is well-formed but not a registered Amplify app and the environment/job were never created.
3. Observe the response: HTTP 400 with body $.code 'ValidationException' and an 'Invalid appId passed.' message.
4. Expected: HTTP 404 with body $.code 'ResourceNotFoundException' and the service's prefixed 'Unable to find matching job with provided parameters.' message — a request-validation refusal must not be used for state that merely does not resolve.
5. Re-replay amplifybackend-get-backend-job-nonexistent-environment#task#2 to confirm a fix.
