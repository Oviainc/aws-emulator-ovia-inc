Issue-Source: trace-sweep-divergence
Issue-Trace: amplifybackend-errors-create-backend-app-not-found
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplifybackend-errors-create-backend-app-not-found amplifybackend-errors-sdk-deprecated-ops amplifybackend-get-backend-job-nonexistent-environment
Issue-Shape-Id: com.amazonaws.amplifybackend#AmplifyBackend
Issue-Key: amplifybackend|CreateBackend|4xx->2xx|$.code
Issue-Key: amplifybackend|CreateBackend|4xx->2xx|$.message
Issue-Key: amplifybackend|CreateBackend|4xx->2xx|$headers.content-type
Issue-Key: amplifybackend|CreateBackend|4xx->2xx|$headers.etag
Issue-Key: amplifybackend|CreateBackend|4xx->2xx|status
Issue-Key: amplifybackend|CreateBackendAuth|4xx->4xx|$.code
Issue-Key: amplifybackend|CreateBackendAuth|4xx->4xx|$.message
Issue-Key: amplifybackend|CreateBackendAuth|4xx->4xx|$headers.content-type
Issue-Key: amplifybackend|CreateBackendAuth|4xx->4xx|$headers.etag
Issue-Key: amplifybackend|CreateBackendAuth|4xx->4xx|status
Issue-Related: 20261022T142056Z-amplifybackend-accepts-unregistered-appids-resource-not-found-or.md
Issue-Commit: c73ebf67a23d923b166243bec836d38257c16c68

# AmplifyBackend CreateBackend/CreateBackendAuth/GetBackendJob error contract: wrong status and missing error code

> _Note: this issue was identified against the code base at commit `c73ebf67a23d923b166243bec836d38257c16c68`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackend, CreateBackendAuth, GetBackendJob

## Description
Three root causes, 3 wire divergence clusters across 3 fixtures: the emulator answers the wrong HTTP status, and never the right error code, on the AmplifyBackend write and job-read error paths. Each cause is a separate fix.

NOTE ON AN EXISTING ISSUE: the open behavioral issue 20261022T052247Z-amplifybackend-control-plane-7-fidelity-divergences-no-environment.md describes the same underlying behaviors for Causes 1-3 from the BEHAVIORAL corpus, but it is tagged Issue-Corpus: simparity-behavioral-aws-v1 and carries no wire replay key; the replay reported owned_by=[] for all three clusters below, so these fixture steps are not covered by it and a fix validated only against that issue will not be exercised by them. The wire evidence here additionally pins the exact statuses, codes and envelope the recordings require.

=== Cause 1 of 3: CreateBackend against a non-existent app returns 200 instead of 404 NotFoundException ===
AWS: Smithy models NotFoundException at HTTP 404 on CreateBackend (POST /backend; errors BadRequestException 400 / GatewayTimeoutException 504 / NotFoundException 404 / TooManyRequestsException 429), and AppId/AppName/BackendEnvironmentName are required bare __strings. The recording shows app identity validated BEFORE any environment work: CreateBackend for a well-formed but non-existent appId is refused with HTTP 404 and error code `NotFoundException`, message of the form "An error occurred while processing your request: App <appId> not found." (the recorded instance names the appId from the request, so the message RULE is known; the literal text is an example). No app and no environment is created.
Emulator: returns HTTP 200 — it creates the app record on demand instead of validating identity — and the body carries neither message nor code.
Why: a caller that mistypes or reuses a deleted appId gets a successful-looking backend create and a JobId for an app AWS has never heard of; nothing surfaces the identity error, and subsequent reads against real AWS fail where the emulator keeps succeeding.
Fix: CreateBackend's handler/state (src/ops/create_backend.rs and the app lookup in src/state.rs) — resolve the app against the Amplify app registry and refuse with the modeled NotFoundException (404) rather than creating state for an unknown appId.
Traces: amplifybackend-errors-create-backend-app-not-found#task#1.

=== Cause 2 of 3: CreateBackendAuth is not withdrawn from SDK/CLI callers — answers 404 instead of the fixed 400 ValidationException ===
AWS: recorded behavior only; Smithy models CreateBackendAuth as an ordinary 200 operation (POST /backend/{AppId}/auth, output AppId/BackendEnvironmentName/JobId/Operation/Status). The recording shows real AmplifyBackend refusing EVERY SDK/CLI CreateBackendAuth call deterministically and input-independently with HTTP 400 and error code `ValidationException`, message "An error occurred while processing your request: Operation not supported through SDK/AWS CLI. Please use Amplify Studio". Because the refusal is a fixed, service-stated sentence, the message is part of the requirement here, not merely an example.
Emulator: answers HTTP 400? no — it answers HTTP 404 with message 'no backend exists for app daec5a84d3d9f' and no code member: it falls through to its ordinary backend lookup, so the response varies with emulator state instead of being the fixed refusal. (Where the backend DOES exist, the operation succeeds and stores an auth resource AWS would never create.)
Why: a client branching on ValidationException plus the Studio-only guidance message never sees it; it sees a state-dependent 404 instead, so the error is read as "create the backend first and retry" and the retry loop never terminates. The status class is wrong too (400 permanent client refusal vs 404 resource-absent).
Fix: CreateBackendAuth's handler (src/ops/create_backend_auth.rs) — refuse unconditionally with the modeled BadRequestException (400) carrying the ValidationException code and the fixed Studio-only message, before any state lookup, and store nothing. The same withdrawal applies to the other Studio-only siblings this fixture exercises.
Traces: amplifybackend-errors-sdk-deprecated-ops#task#1.

=== Cause 3 of 3: GetBackendJob for an app with no backend reports no error code where AWS reports ResourceNotFoundException ===
AWS: recorded behavior only for the code; Smithy binds GetBackendJob (GET /backend/{AppId}/job/{BackendEnvironmentName}/{JobId}) to BadRequestException/GatewayTimeoutException/NotFoundException/TooManyRequestsException only. The recording shows a GetBackendJob whose (app, environment, job) triple does not resolve answering HTTP 404 with error code `ResourceNotFoundException` and a message of the form "An error occurred while processing your request: Unable to find matching job with provided parameters." — AWS does not distinguish "this app has no backend" from "this job does not exist" on this path, and the code it uses is not one of the modeled shapes' names.
Emulator: the status is right (404) but the body carries NO `code` member at all (expected 'ResourceNotFoundException', got None) and the message is the emulator's own app-lookup text 'no backend exists for app d2n9n0likdcl0a' — the RESOURCE_TYPE_BACKEND not_found branch of the app lookup (the pattern at src/ops/get_backend.rs:160-167, mirrored in get_backend_job.rs) rather than a single job-not-found path. The response's content-type is also 'application/json' instead of the recorded 'application/json; charset=utf-8', and no ETag header is sent (the recorded ETag value is body-derived and asserted shaped-only, so only its presence is the behavior).
Why: clients branch on the error code to decide whether a job is absent or still settling; with no code in the body the failure is unclassifiable, and the app-missing vs job-missing split leaks an emulator-internal distinction AWS does not make.
Fix: GetBackendJob's handler plus the service's error channel (src/ops/get_backend_job.rs and the error constructors in src/state.rs, and the restJson1 error serializer) — collapse the app-missing and job-missing branches into one job-not-found refusal that emits the ResourceNotFoundException code, the service's prefixed message, and the service's content-type/ETag headers.
Traces: amplifybackend-get-backend-job-nonexistent-environment#task#2.

## Evidence
Replay of trace set simparity-wire-aws-v1 (summary: 33 steps, 2 passed, 31 mismatched, score 6.1%). All three clusters reported stub_path="" and owned_by=[].

Cause 1 — fixture amplifybackend-errors-create-backend-app-not-found, step task#1, CreateBackend:
  expected: HTTP 404, $.code 'NotFoundException', $.message 'An error occurred while processing your request: App da57dbb8db056 not found.'
  actual:   HTTP 200, no $.message, no $.code; $headers.content-type 'application/json' (expected 'application/json; charset=utf-8'); $headers.etag absent.

Cause 2 — fixture amplifybackend-errors-sdk-deprecated-ops, step task#1, CreateBackendAuth:
  expected: HTTP 400, $.code 'ValidationException', $.message 'An error occurred while processing your request: Operation not supported through SDK/AWS CLI. Please use Amplify Studio'
  actual:   HTTP 404, $.message 'no backend exists for app daec5a84d3d9f', no $.code; content-type 'application/json'; etag absent.

Cause 3 — fixture amplifybackend-get-backend-job-nonexistent-environment, step task#2, GetBackendJob:
  expected: HTTP 404 (status matched), $.code 'ResourceNotFoundException', $.message 'An error occurred while processing your request: Unable to find matching job with provided parameters.'
  actual:   $.code missing (got None), $.message 'no backend exists for app d2n9n0likdcl0a'; content-type 'application/json' (expected 'application/json; charset=utf-8'); etag missing (expected shaped, not asserted exactly).

Model citations: CreateBackend / CreateBackendAuth / GetBackendJob each bind BadRequestException(400), GatewayTimeoutException(504), NotFoundException(404), TooManyRequestsException(429); BadRequestException models only Message (jsonName 'message'); NotFoundException models Message + ResourceType. No `code` member and neither ValidationException nor ResourceNotFoundException exist in the model, so those codes are recorded behavior only.

## Steps to Reproduce
Boot the emulator and replay these three fixture steps:

1. amplifybackend-errors-create-backend-app-not-found, task#1 — POST /backend with AppId set to a well-formed appId that is not a registered Amplify app (e.g. 'da57dbb8db056'), AppName and BackendEnvironmentName set. Observe HTTP 200 with a JobId instead of HTTP 404 with code NotFoundException and an 'App <appId> not found.' message (Cause 1).
2. amplifybackend-errors-sdk-deprecated-ops, task#1 — POST /backend/{appId}/auth (CreateBackendAuth) with a valid-looking ResourceConfig/ResourceName. Observe HTTP 404 'no backend exists for app <appId>' instead of the fixed HTTP 400 ValidationException 'Operation not supported through SDK/AWS CLI. Please use Amplify Studio' (Cause 2).
3. amplifybackend-get-backend-job-nonexistent-environment, task#2 — GET /backend/{appId}/job/{neverCreatedEnv}/{neverCreatedJob} (GetBackendJob). Status is 404 as expected, but the body carries no `code` member where ResourceNotFoundException is required, and the message is the app-lookup text (Cause 3).
4. On each response, also inspect the headers: content-type is 'application/json' rather than 'application/json; charset=utf-8' and no ETag is set.
