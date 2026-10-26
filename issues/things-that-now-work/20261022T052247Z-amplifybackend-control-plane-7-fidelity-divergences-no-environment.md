Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplifybackend.create-backend-nonexistent-app-not-found amplifybackend.create-backend-auth-job-lifecycle amplifybackend.clone-backend-duplicate-target-rejected amplifybackend.get-backend-job-nonexistent-environment-not-found amplifybackend.delete-backend-job-reaches-succeeded amplifybackend.get-backend-lists-created-environment
Issue-Shape-Id: com.amazonaws.amplifybackend#AmplifyBackend
Issue-Key: amplifybackend.create-backend-nonexistent-app-not-found|_client_error#6|AssertionError|raise AssertionError(f"expected a ClientError; call succeeded unexpectedly with {result!r}")
Issue-Key: amplifybackend.create-backend-auth-job-lifecycle|run#12|AssertionError|raise AssertionError(
Issue-Key: amplifybackend.clone-backend-duplicate-target-rejected|run#11|AssertionError|assert error_fresh.get("Code") == "ValidationException", (
Issue-Key: amplifybackend.get-backend-job-nonexistent-environment-not-found|run#4|AssertionError|assert error["Code"] == "ResourceNotFoundException", repr(error)
Issue-Key: amplifybackend.delete-backend-job-reaches-succeeded|run#16|AssertionError|assert error.get("Message") == (
Issue-Key: amplifybackend.get-backend-lists-created-environment|run#8|AssertionError|assert job["Status"] == "COMPLETED", f"backend job did not complete: {job}"
Issue-Commit: f2a9c91c50f7e55687472cf5b456a197c6c3ca83

# AmplifyBackend control plane: 7 fidelity divergences (no environment-name/app-identity validation, CreateBackendAuth not withdrawn, CloneBackend 501, wrong not-found error code, message prefix rule, job status vocabulary)

> _Note: this issue was identified against the code base at commit `f2a9c91c50f7e55687472cf5b456a197c6c3ca83`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackend, CreateBackendAuth, CloneBackend, GetBackendJob, DeleteBackend

## Description
Seven distinct root causes in the AmplifyBackend emulator, all grounded in failing behavioral checks of this suite. The AmplifyBackend Smithy model types almost everything as a bare `__string` and models only BadRequestException/GatewayTimeoutException/NotFoundException/TooManyRequestsException, so most of the required behavior below is "recorded behavior only" — the check bodies state what a real-AWS replay observed, and that is what is cited.

=== Cause 1 of 7: CreateBackend performs no BackendEnvironmentName format validation ===
AWS: recorded behavior only. Smithy: CreateBackend.BackendEnvironmentName targets `__string` with no @length/@pattern, and the operation models no ValidationException at all. The recording shows real AWS refusing a BackendEnvironmentName outside 2-10 lowercase LETTERS with HTTP 400, error code `ValidationException`, message built from the broken rule, e.g. "An error occurred while processing your request: Environment name must be between 2 and 10 characters, and lowercase only." (three independent checks in this suite document the same constraint).
Emulator: returns HTTP 200 and starts a job for BackendEnvironmentName="env-name-too-long-to-be-valid" (29 chars, hyphens): `{'AppId': 'd32oq2eb6b3gta', 'BackendEnvironmentName': 'env-name-too-long-to-be-valid', 'JobId': ..., 'Operation': 'CreateBackend', 'Status': 'IN_PROGRESS'}`. The only caller-string check in the handler is the emulator's own 256-char resource guard.
Why: a client that relies on AWS rejecting a malformed environment name gets a provisioned-looking environment under a name AWS could never create; naming bugs in client code are silently accepted and only surface against real AWS.
Fix: CreateBackend's input validation (the handler's pre-insert guard block), refusing with a 400 ValidationException whose message names the member and the constraint.
Traces: amplifybackend.create-backend-nonexistent-app-not-found

=== Cause 2 of 7: CreateBackend auto-creates an app record for an AppId that is not a real Amplify app ===
AWS: recorded behavior only. The recording shows CreateBackend against a well-formed but non-existent AppId refused with error code `NotFoundException` (modeled, HTTP 404) and a message of the form "An error occurred while processing your request: App <appId> not found." App identity is validated before any environment work.
Emulator: there is no app-existence check — the handler calls `app_or_create(&input.app_id, &input.app_name)`, so any AppId brings a brand-new app record into being and the call succeeds. This is the second assertion of the same check and is reached only once Cause 1 is fixed, so it must be closed together with it.
Why: a caller that mistypes or uses a deleted appId gets a successful backend create against an app that does not exist; nothing surfaces the identity error.
Fix: CreateBackend's handler/state — validate the app exists (against the Amplify app registry) before creating state, and refuse with the modeled NotFoundException (404).
Traces: amplifybackend.create-backend-nonexistent-app-not-found

=== Cause 3 of 7: CreateBackendAuth succeeds where AWS has withdrawn it from SDK/CLI callers ===
AWS: recorded behavior only. Smithy models CreateBackendAuth as an ordinary 200 operation (output AppId/JobId/Operation/Status), but the recording shows real AWS refusing EVERY SDK/CLI CreateBackendAuth call deterministically with HTTP 400, error code `ValidationException`, and the fixed message "An error occurred while processing your request: Operation not supported through SDK/AWS CLI. Please use Amplify Studio" — the refusal is input-independent. A subsequent GetBackendAuth for that resource then reports error code `ResourceNotFoundException` (example message: "An error occurred while processing your request: Amplify auth parameters file not found."), i.e. the create left no state.
Emulator: CreateBackendAuth returns 200 and stores the auth resource (the check fails with "CreateBackendAuth succeeded through the SDK").
Why: an overly permissive success. A client gets a JobId for an auth resource that real AWS would never create, so an integration that passes against the emulator fails outright against AWS, and emulator state carries a Cognito-backed resource that cannot exist.
Fix: CreateBackendAuth's handler — refuse unconditionally with the 400 ValidationException refusal, store nothing. (The same SDK/CLI withdrawal applies to the Studio-only sibling operations exercised elsewhere in this suite.)
Traces: amplifybackend.create-backend-auth-job-lifecycle

=== Cause 4 of 7: CloneBackend answers 501 NotImplemented instead of AWS's fixed refusal ===
AWS: recorded behavior only. The recording shows every CloneBackend call — fresh TargetEnvironmentName or one already in use — refused identically with HTTP 400, error code `ValidationException`, and the fixed message 'An error occurred while processing your request: Operation not supported through SDK/AWS CLI. Please use "amplify pull --appId <appId> --envName staging && amplify add env && amplify push" from Amplify CLI'. The refusal precedes any target-name validation, and no environment is created.
Emulator: `services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/clone_backend.rs` is still the scaffold stub (`Scaffolded::not_implemented("CloneBackend")`), so the call answers error code `NotImplemented` (501). The check caught it as a ClientError rather than a crash, so the suite classified it a mismatch and did NOT mark it already-filed; no open issue names CloneBackend.
Why: the client sees a server-side "not implemented" instead of the permanent client-side refusal, so error-handling code that branches on ValidationException (and on the CLI guidance message) never runs, and a retry loop treats 501 as transient.
Fix: implement clone_backend.rs as the fixed 400 ValidationException refusal (no clone semantics are needed), per the file's own guidance that 501 means "not written yet" and not "refused".
Traces: amplifybackend.clone-backend-duplicate-target-rejected

=== Cause 5 of 7: GetBackendJob returns NotFoundException where AWS returns ResourceNotFoundException ===
AWS: recorded behavior only. Smithy binds GetBackendJob only to BadRequestException/GatewayTimeoutException/NotFoundException/TooManyRequestsException, but the recording shows a GetBackendJob for a live app whose backend environment/job was never created answering with error code `ResourceNotFoundException` (example message: "An error occurred while processing your request: Unable to find matching job with provided parameters."). AWS does not distinguish "this app has no backend" from "this job does not exist" on this path.
Emulator: answers `{'Code': 'NotFoundException', 'Message': 'no backend exists for app d128e3b4z3iz9b'}` — the wrong error code, produced by the app-lookup branch in `get_backend_job.rs` (the `RESOURCE_TYPE_BACKEND` not_found) rather than a single job-not-found path.
Why: clients branch on the error code; code catching ResourceNotFoundException to poll-until-ready or to treat a job as absent instead sees an unexpected NotFoundException and raises. Same applies to the GetBackendAuth read in Cause 3.
Fix: GetBackendJob's handler and the service's error channel — emit the `ResourceNotFoundException` code for an unresolvable (app, environment, job) triple, collapsing the app-missing and job-missing branches.
Traces: amplifybackend.get-backend-job-nonexistent-environment-not-found

=== Cause 6 of 7: AmplifyBackend error messages are emulator-invented and omit the service's fixed message prefix ===
AWS: the message RULE is known from the recordings — every AmplifyBackend error message is prefixed "An error occurred while processing your request: " followed by the service's own sentence, and where it names a resource it names it from the request. For a second DeleteBackend on an environment already removed, AWS answers error code `NotFoundException` (which the emulator gets right) with a message of the form "An error occurred while processing your request: BackendEnvironment <name> does not exist."
Emulator: `delete_backend.rs` builds its own text, 'backend environment fofimglf does not exist for app d1cch6klbbj8os' — no prefix, different sentence, and it appends the appId. The same locally-invented wording is used throughout `src/state.rs` (`not_found`, `limit_exceeded`, `bound_identifier`), so this is one cause across the service, not one per operation.
Why: a client (or a log/alarm pattern) that matches on the documented prefix or sentence never matches; the message is the only place AmplifyBackend conveys which rule was broken, since the error codes are coarse.
Fix: the service's error construction in `src/state.rs` plus the per-operation not-found sites — apply the fixed prefix and the service's sentence form rather than ad-hoc text.
Traces: amplifybackend.delete-backend-job-reaches-succeeded

=== Cause 7 of 7: job Status uses an invented in-flight vocabulary (IN_PROGRESS) ===
AWS: recorded behavior only — GetBackendJob.Status is an untyped `__string` in the model. The recording observed a fresh CreateBackend job reading back as `PENDING` or `RUNNING` while in flight and settling on `COMPLETED` (with no Error) on success; `FAILED` is the failure terminal.
Emulator: `src/state.rs` defines the in-flight value as `JOB_STATUS_IN_PROGRESS = "IN_PROGRESS"` and reports it until a chosen 500ms delay elapses, e.g. `{'Operation': 'CreateBackend', 'Status': 'IN_PROGRESS', ...}`.
Why: a client that polls AWS's vocabulary — "keep polling while Status is PENDING or RUNNING" — treats IN_PROGRESS as a terminal state on the first read and concludes the job neither completed nor failed; the check fails with "backend job did not complete: ... 'Status': 'IN_PROGRESS'". Any state machine keyed on the AWS values mis-routes.
Fix: the job state model in `src/state.rs` (status constants and `BackendJob::settle`) so an unsettled job reports PENDING/RUNNING and a settled one COMPLETED/FAILED; the reporting handlers then need no change.
Traces: amplifybackend.get-backend-lists-created-environment

## Evidence
Suite: 1/15 passed for amplifybackend; 6 mismatches clustered here (the 8 errored checks are blocked by already-filed CreateToken/ListBackendJobs/ListS3Buckets/RemoveAllBackends stubs and are NOT part of this issue).

Cause 1 — amplifybackend.create-backend-nonexistent-app-not-found:
  expected: ClientError, Code=ValidationException, Message="An error occurred while processing your request: Environment name must be between 2 and 10 characters, and lowercase only."
  actual:   HTTP 200 {'AppId': 'd32oq2eb6b3gta', 'BackendEnvironmentName': 'env-name-too-long-to-be-valid', 'JobId': '1792646289227', 'Operation': 'CreateBackend', 'Status': 'IN_PROGRESS'}

Cause 2 — same check, assertion after Cause 1:
  expected: Code=NotFoundException, Message="An error occurred while processing your request: App <bogusAppId> not found."
  actual:   not reached; handler calls state::app_or_create, so an unknown AppId is created and the call succeeds.

Cause 3 — amplifybackend.create-backend-auth-job-lifecycle:
  expected: ClientError Code=ValidationException, Message="An error occurred while processing your request: Operation not supported through SDK/AWS CLI. Please use Amplify Studio"; then GetBackendAuth -> ResourceNotFoundException
  actual:   AssertionError: "CreateBackendAuth succeeded through the SDK; AWS has apparently restored SDK/CLI support and this withdrawal check needs re-auditing" (HTTP 200)

Cause 4 — amplifybackend.clone-backend-duplicate-target-rejected:
  expected: Code=ValidationException (HTTP 400) for both a fresh and an in-use TargetEnvironmentName
  actual:   AssertionError: "CloneBackend into a fresh target name was rejected with 'NotImplemented', expected 'ValidationException'" (501 scaffold stub in src/ops/clone_backend.rs)

Cause 5 — amplifybackend.get-backend-job-nonexistent-environment-not-found:
  expected: Code=ResourceNotFoundException
  actual:   {'Message': 'no backend exists for app d128e3b4z3iz9b', 'Code': 'NotFoundException'}

Cause 6 — amplifybackend.delete-backend-job-reaches-succeeded:
  expected: Code=NotFoundException with "An error occurred while processing your request: BackendEnvironment <env> does not exist."
  actual:   {'Message': 'backend environment fofimglf does not exist for app d1cch6klbbj8os', 'Code': 'NotFoundException'} (code correct, message rule wrong)

Cause 7 — amplifybackend.get-backend-lists-created-environment:
  expected: in-flight Status in (PENDING, RUNNING), terminal COMPLETED
  actual:   AssertionError: "backend job did not complete: {... 'JobId': '1792646284976', 'Operation': 'CreateBackend', 'Status': 'IN_PROGRESS'}"

## Steps to Reproduce
Boot the emulator and replay these amplifybackend behavioral checks:

1. amplifybackend.create-backend-nonexistent-app-not-found — create a real Amplify app, then CreateBackend(AppId=<live>, AppName=<any>, BackendEnvironmentName="env-name-too-long-to-be-valid"); observe HTTP 200 instead of 400 ValidationException (Cause 1). Then CreateBackend(AppId="d"+random hex, BackendEnvironmentName="dev"); observe success instead of 404 NotFoundException (Cause 2).
2. amplifybackend.create-backend-auth-job-lifecycle — CreateBackend a valid 2-10 lowercase-letter environment, poll GetBackendJob to COMPLETED, then CreateBackendAuth(AppId, BackendEnvironmentName, ResourceName="cognitoauth", ResourceConfig={AuthResources: USER_POOL_ONLY, Service: COGNITO, UserPoolConfigs: {...}}); observe 200 instead of the fixed 400 ValidationException (Cause 3).
3. amplifybackend.clone-backend-duplicate-target-rejected — CloneBackend(AppId, BackendEnvironmentName="enva", TargetEnvironmentName="envb"); observe Code=NotImplemented instead of ValidationException (Cause 4).
4. amplifybackend.get-backend-job-nonexistent-environment-not-found — GetBackendJob(AppId=<live app with no backend>, BackendEnvironmentName="never-created-env", JobId="never-created-job"); observe Code=NotFoundException instead of ResourceNotFoundException (Cause 5).
5. amplifybackend.delete-backend-job-reaches-succeeded — CreateBackend, wait for COMPLETED, DeleteBackend, wait for COMPLETED, then DeleteBackend again; inspect the NotFoundException message (Cause 6).
6. amplifybackend.get-backend-lists-created-environment — CreateBackend and read GetBackendJob immediately; observe Status=IN_PROGRESS where AWS reports PENDING/RUNNING (Cause 7).
