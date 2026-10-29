Issue-Source: trace-sweep-divergence
Issue-Trace: amplifybackend-errors-unregistered-appid
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplifybackend-errors-unregistered-appid
Issue-Shape-Id: com.amazonaws.amplifybackend#AmplifyBackend
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|$.code
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|$.message
Issue-Key: amplifybackend|GetBackendJob|4xx->4xx|status
Issue-Commit: c4171394ecd9e8acdff90bddf637ffaae1174b81

# AmplifyBackend GetBackendJob does not reject an unregistered appId with 400 ValidationException

> _Note: this issue was identified against the code base at commit `c4171394ecd9e8acdff90bddf637ffaae1174b81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBackendJob

## Description
One root cause, 1 wire divergence cluster: AmplifyBackend's GetBackendJob path never validates the APP IDENTITY in the {AppId} URI label, so a request naming an appId that is not a registered Amplify app is answered as an ordinary job lookup miss instead of as an input-validation refusal.

NOTE ON AN EXISTING ISSUE: the open issue 20261022T142056Z-amplifybackend-createbackend-createbackendauth-getbackendjob-error-contract-wrong-status.md covers GetBackendJob's job-not-found body (its Cause 3: the missing `code` member and the app-lookup message text) against a DIFFERENT fixture step (amplifybackend-get-backend-job-nonexistent-environment#task#2). That cause now appears satisfied here — the emulator does emit code ResourceNotFoundException and the service-prefixed job-not-found message. This issue is the distinct, still-open gap: the unregistered-appId path, which must not reach the job lookup at all. The replay reported owned_by=[] and stub_path="" for this cluster, so it is not covered by that issue and a fix validated only against it will not be exercised by this step.

=== Cause 1 of 1: unregistered appId answered 404 ResourceNotFoundException instead of 400 ValidationException 'Invalid appId passed.' ===
AWS: Smithy binds GetBackendJob to GET /backend/{AppId}/job/{BackendEnvironmentName}/{JobId} with AppId/BackendEnvironmentName/JobId as required bare __string httpLabels, and models errors BadRequestException(400), GatewayTimeoutException(504), NotFoundException(404), TooManyRequestsException(429); BadRequestException models only Message (jsonName 'message') and the model has no `code` member, so the wire error code is recorded behavior only. The recording shows AWS resolving and validating the appId BEFORE any environment/job lookup: when the appId is not a registered Amplify app, the call is refused with HTTP 400 and error code `ValidationException`, message "An error occurred while processing your request: Invalid appId passed." The fixed text of that message is the whole sentence including the service's "An error occurred while processing your request: " prefix — it carries no values from the request or from state, so nothing in it is a per-run value. The job-not-found refusal (HTTP 404, code `ResourceNotFoundException`, fixed message "An error occurred while processing your request: Unable to find matching job with provided parameters.") is the distinct, lower-precedence outcome AWS uses only once the app resolves.
Emulator: answers HTTP 404 with code `ResourceNotFoundException` and the job-not-found message "An error occurred while processing your request: Unable to find matching job with provided parameters." — i.e. it skips app-identity validation entirely and reports every unresolvable triple, including one whose app does not exist, through the single job-miss branch. Wrong status class (404 resource-absent vs 400 permanent client input error), wrong error code, wrong message.
Why: a client that mistypes, truncates or reuses a deleted appId is told the JOB is missing rather than that its appId is invalid. Because 404/ResourceNotFoundException reads as "the job is not there yet", SDK and CLI callers poll or retry the same bad appId indefinitely instead of surfacing a permanent validation error, and code branching on ValidationException to re-prompt for an app never fires. It also hides the identity error behind a state-dependent answer: the same request would start succeeding if a job with that id happened to exist.
Fix: GetBackendJob's validation/handler, ahead of the job lookup in the operation handler and the app resolution it calls into in the service's state layer — resolve {AppId} against the Amplify app registry first and refuse an unregistered appId with the modeled BadRequestException (HTTP 400) carrying the ValidationException code and the fixed 'Invalid appId passed.' message; only reach the ResourceNotFoundException job-miss branch once the app resolves. The same appId precedence applies to the sibling job/backend read operations that take the {AppId} label.
Traces: amplifybackend-errors-unregistered-appid#task#3.

## Evidence
Replay of trace set simparity-wire-aws-v1 (summary: 33 steps total, 16 passed, 17 mismatched, 0 errored, 0 blocked, score 48.5%, bootstrap_signal false, divergence_count 1). The cluster reported stub_path="" and owned_by=[], affected_count 1.

Fixture amplifybackend-errors-unregistered-appid, step task#3, operation GetBackendJob, verdict mismatch, divergence_key amplifybackend|GetBackendJob|status=400->404|keys=$.code,$.message,status:
  expected: HTTP 400, $.code 'ValidationException', $.message 'An error occurred while processing your request: Invalid appId passed.'
  actual:   HTTP 404, $.code 'ResourceNotFoundException', $.message 'An error occurred while processing your request: Unable to find matching job with provided parameters.'

Message anatomy: the expected message is entirely fixed text (prefix + 'Invalid appId passed.') with no request- or state-derived values; the actual message is the fixed job-not-found sentence, also fully fixed text. So the divergence is in status, code and the choice of refusal, not in any per-run value.

Model citations: GetBackendJob GET /backend/{AppId}/job/{BackendEnvironmentName}/{JobId}, AppId required httpLabel __string; errors BadRequestException(400) / GatewayTimeoutException(504) / NotFoundException(404) / TooManyRequestsException(429); BadRequestException models only Message (jsonName 'message'); neither ValidationException nor ResourceNotFoundException exists as a modeled shape, so both wire codes are recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay fixture amplifybackend-errors-unregistered-appid, step task#3:

1. GET /backend/{AppId}/job/{BackendEnvironmentName}/{JobId} (GetBackendJob) where {AppId} is a well-formed but UNREGISTERED Amplify app id (no app of that id has ever been created in the emulator), and the environment name and job id are likewise arbitrary.
2. Observe the response: HTTP 404 with body code 'ResourceNotFoundException' and message 'An error occurred while processing your request: Unable to find matching job with provided parameters.'
3. Expected per the recording: HTTP 400 with body code 'ValidationException' and message 'An error occurred while processing your request: Invalid appId passed.'
4. For contrast, create an app/backend and then call GetBackendJob with a nonexistent job id under it: that case SHOULD give the 404 ResourceNotFoundException job-not-found refusal, confirming the app-identity check must run first and take precedence.
