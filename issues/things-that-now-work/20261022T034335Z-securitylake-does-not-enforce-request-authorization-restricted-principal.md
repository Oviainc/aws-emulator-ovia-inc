Issue-Source: trace-sweep-divergence
Issue-Trace: securitylake-iam-policy-restricted-ops-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: securitylake-iam-policy-restricted-ops-error-paths
Issue-Shape-Id: com.amazonaws.securitylake#SecurityLake
Issue-Key: securitylake|GetSubscriber|4xx->4xx|$.message
Issue-Key: securitylake|GetSubscriber|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securitylake|GetSubscriber|4xx->4xx|status
Issue-Key: securitylake|DeleteDataLake|4xx->4xx|$.message
Issue-Key: securitylake|DeleteDataLake|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securitylake|DeleteDataLake|4xx->4xx|status
Issue-Key: securitylake|DeleteSubscriber|4xx->4xx|$.message
Issue-Key: securitylake|DeleteSubscriber|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: securitylake|DeleteSubscriber|4xx->4xx|status
Issue-Commit: 37e4b54b7f2f231778aa0090f22742466dbbad1b

# securitylake does not enforce request authorization: restricted-principal operations return resource errors instead of 401

> _Note: this issue was identified against the code base at commit `37e4b54b7f2f231778aa0090f22742466dbbad1b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSubscriber

## Description
The emulator performs no authorization/authentication decision for Security Lake requests. Every request is admitted to the operation handler, which then answers from state — so a caller whose principal is not permitted to invoke the operation receives a resource-level error (or a success) instead of the authorization failure the real service returns. AWS evaluates authorization BEFORE resource lookup, so the authorization outcome is independent of whether the named resource exists or whether Security Lake is enabled; the emulator's answer leaks resource state to a principal that is not allowed to ask. Three operations in one recorded error-path fixture show the same single root cause.

Cause 1 of 3: GetSubscriber on a request from a restricted principal.
  AWS:      Smithy: GetSubscriber GET /v1/subscribers/{subscriberId}; modeled client errors are AccessDeniedException (403), BadRequestException (400), ConflictException (409), ResourceNotFoundException (404), ThrottlingException (429). The model does NOT model a 401, so for the authorization-refusal status this is recorded behavior only: the recording shows the service rejecting the call with HTTP 401 and body message "Unauthorized" (example text) and asserting no x-amzn-errortype, i.e. the request is refused at the front door before the subscriber is ever looked up.
  Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and message "no subscriber with id 00000000-0000-0000-0000-000000000000 exists in Region us-west-2" (example text) — the handler ran and reported on state.
  Why:      A client cannot distinguish "you may not do this" from "it is not there". SDK retry/credential-refresh logic keys off 401/403 (re-sign, refresh credentials, surface a permissions error); a 404 ResourceNotFoundException is treated as a definitive negative answer, so permission misconfiguration is silently reported as a missing subscriber and callers that probe for existence are told about resources they are not authorized to see.
  Fix:      Request-admission/authorization layer in front of the securitylake operation dispatch (not inside GetSubscriber's handler): decide the authorization outcome for the signing principal and return the refusal status before resource lookup.
  Traces:   securitylake-iam-policy-restricted-ops-error-paths#task#1.

Cause 2 of 3: DeleteDataLake on a request from a restricted principal.
  AWS:      Smithy: DeleteDataLake POST /v1/datalake/delete, input regions (required, unique); modeled client errors AccessDeniedException (403), BadRequestException (400), ConflictException (409), ResourceNotFoundException (404), ThrottlingException (429). 401 is not modeled — recorded behavior only: the recording shows HTTP 401 with message "Unauthorized" (example text) and no asserted x-amzn-errortype, before any data-lake state is consulted.
  Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and message "Security Lake is not enabled in Region us-west-2" (example text).
  Why:      A mutating call that must be refused is instead reported as a state problem, so a caller (or a teardown script) concludes the data lake is already gone and continues; the authorization failure never surfaces.
  Fix:      Same authorization layer ahead of securitylake dispatch; the DeleteDataLake handler's enablement check must only run for an authorized principal.
  Traces:   securitylake-iam-policy-restricted-ops-error-paths#task#4.

Cause 3 of 3: DeleteSubscriber on a request from a restricted principal.
  AWS:      Smithy: DeleteSubscriber DELETE /v1/subscribers/{subscriberId}, idempotent; modeled client errors AccessDeniedException (403), BadRequestException (400), ConflictException (409), ResourceNotFoundException (404), ThrottlingException (429). 401 is not modeled — recorded behavior only: the recording shows HTTP 401, message "Unauthorized" (example text), no asserted x-amzn-errortype.
  Emulator: HTTP 404 with x-amzn-errortype: ResourceNotFoundException and message "no subscriber with id 00000000-0000-0000-0000-000000000000 exists in Region us-west-2" (example text).
  Why:      Same as cause 1, on a mutating path: a refused delete is reported as an absent subscriber, so callers treat the delete as already satisfied.
  Fix:      Same authorization layer ahead of securitylake dispatch.
  Traces:   securitylake-iam-policy-restricted-ops-error-paths#task#12.

All three statuses and the absence of an error-type header are requirements; the "Unauthorized" message text above is an example of the recorded body, not a required string. No prior securitylake issue exists (list_existing_issues query 'securitylake' returned nothing), and these steps carried no stub_path and no owned_by.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture securitylake-iam-policy-restricted-ops-error-paths (3 diverging steps, verdict=mismatch, each a 1-member cluster):

- task#1 GetSubscriber — expected status 401, got 404. $.message value_mismatch (expected 'Unauthorized', got 'no subscriber with id 00000000-0000-0000-0000-000000000000 exists in Region us-west-2'). $headers.x-amzn-errortype shape_mismatch (expected <shaped-not-asserted>, got 'ResourceNotFoundException'). divergence_key service=securitylake|GetSubscriber|status=401->404.
- task#4 DeleteDataLake — expected status 401, got 404. $.message value_mismatch (expected 'Unauthorized', got 'Security Lake is not enabled in Region us-west-2'). $headers.x-amzn-errortype shape_mismatch (expected <shaped-not-asserted>, got 'ResourceNotFoundException'). divergence_key service=securitylake|DeleteDataLake|status=401->404.
- task#12 DeleteSubscriber — expected status 401, got 404. $.message value_mismatch (expected 'Unauthorized', got 'no subscriber with id 00000000-0000-0000-0000-000000000000 exists in Region us-west-2'). $headers.x-amzn-errortype shape_mismatch (expected <shaped-not-asserted>, got 'ResourceNotFoundException'). divergence_key service=securitylake|DeleteSubscriber|status=401->404.

Common shape of the divergence: expected = authorization refusal (4xx auth status, no x-amzn-errortype), actual = resource-state error produced by the operation handler. state_reset_failures=0 and emulator_up=true for this replay, so the results are not contaminated by leftover state or a dead target.

Smithy model consulted: services/service-com-amazonaws-securitylake-securitylake/smithy-model/com-amazonaws-securitylake-securitylake.json — GetSubscriber, DeleteSubscriber, DeleteDataLake all model AccessDeniedException(403)/BadRequestException(400)/ConflictException(409)/ResourceNotFoundException(404)/ThrottlingException(429); no 401 shape, hence the 401 is grounded in the recording only.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the wire corpus (trace set simparity-wire-aws-v1).
3. Fixture securitylake-iam-policy-restricted-ops-error-paths, step task#1: issue GetSubscriber GET /v1/subscribers/00000000-0000-0000-0000-000000000000 as the fixture's restricted principal. Expected: HTTP 401 authorization refusal with no x-amzn-errortype. Actual: HTTP 404 with x-amzn-errortype: ResourceNotFoundException.
4. Same fixture, step task#4: POST /v1/datalake/delete with the recorded regions list. Expected HTTP 401; actual HTTP 404 ResourceNotFoundException ("Security Lake is not enabled...").
5. Same fixture, step task#12: DELETE /v1/subscribers/00000000-0000-0000-0000-000000000000. Expected HTTP 401; actual HTTP 404 ResourceNotFoundException.
6. After the fix, re-replay fixture securitylake-iam-policy-restricted-ops-error-paths and confirm all three steps (task#1, task#4, task#12) pass.
