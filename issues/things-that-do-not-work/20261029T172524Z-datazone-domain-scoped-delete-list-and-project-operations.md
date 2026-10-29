Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-domain-scoped-denied-sweep-10
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-domain-scoped-denied-sweep-10 datazone-domain-scoped-denied-sweep-9 datazone-domain-scoped-denied-sweep-2 datazone-domain-scoped-denied-sweep-7 datazone-domain-lifecycle
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|DeleteProject|4xx->4xx|$.message
Issue-Key: datazone|DeleteProject|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|DeleteProject|4xx->4xx|status
Issue-Key: datazone|DeleteProjectMembership|4xx->4xx|$.message
Issue-Key: datazone|DeleteProjectMembership|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|DeleteProjectMembership|4xx->4xx|status
Issue-Key: datazone|DeleteFormType|4xx->4xx|$.message
Issue-Key: datazone|DeleteFormType|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|DeleteFormType|4xx->4xx|status
Issue-Key: datazone|ListProjectMemberships|4xx->4xx|$.message
Issue-Key: datazone|ListProjectMemberships|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|ListProjectMemberships|4xx->4xx|status
Issue-Key: datazone|GetProject|4xx->4xx|$.message
Issue-Key: datazone|GetProject|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetProject|4xx->4xx|status
Issue-Key: datazone|DeleteDomain|4xx->4xx|$
Issue-Key: datazone|DeleteDomain|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|DeleteDomain|4xx->4xx|status
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# datazone domain-scoped delete, list and project operations answer 404 instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteProject

## Description
SIX causes, one shared fault, same ordering defect as it affects mutating and collection operations: when the caller is not authorized on the domain, datazone denies the call before resolving the domain or the child resource, so the answer is 403 AccessDeniedException; the emulator resolves state first and answers 404 ResourceNotFoundException. Fix all six before submitting.

Shared AWS behavior (Smithy): these operations bind AccessDeniedException (httpError 403, message @required) and, where applicable, ResourceNotFoundException (httpError 404). The recorded denial is 403, x-amzn-errortype AccessDeniedException, message `User is not permitted to perform operation: <OperationName>` — fixed text plus the request's operation name, with nothing taken from the request path. Shared emulator behavior: 404, x-amzn-errortype ResourceNotFoundException, an existence message. Shared Why: a delete that is actually forbidden reports as "already gone", so idempotent-delete and clean-up logic records success and a caller silently loses the permissions signal; list callers see an empty/missing resource rather than a denial. Shared Fix: datazone's domain-scoped authorization pre-check must run ahead of the handlers' state lookups, and must build AccessDeniedException with AWS's message.

Cause 1 of 6: DeleteProject. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: DeleteProject`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-10#task#5.

Cause 2 of 6: DeleteProjectMembership. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: DeleteProjectMembership`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-10#task#11.

Cause 3 of 6: DeleteFormType. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: DeleteFormType`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-9#task#12.

Cause 4 of 6: ListProjectMemberships. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: ListProjectMemberships`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-2#task#9.

Cause 5 of 6: GetProject. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetProject`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-7#task#3.

Cause 6 of 6: DeleteDomain. AWS: 403 with x-amzn-errortype AccessDeniedException and an EMPTY JSON body in the recording ({}), i.e. the denial carries no additional members beyond what the recording shows. Emulator: 404 with x-amzn-errortype ResourceNotFoundException and body {"__type": "ResourceNotFoundException", "message": "Domain not found"}. Traces: datazone-domain-lifecycle#task#5.

## Evidence
Replay report, expected -> actual:
1. DeleteProject 403 -> 404; $.message expected 'User is not permitted to perform operation: DeleteProject', got 'Domain not found'; x-amzn-errortype got 'ResourceNotFoundException'.
2. DeleteProjectMembership 403 -> 404; expected 'User is not permitted to perform operation: DeleteProjectMembership', got 'Domain not found'.
3. DeleteFormType 403 -> 404; expected 'User is not permitted to perform operation: DeleteFormType', got 'Domain not found'; errortype expected 'AccessDeniedException', got 'ResourceNotFoundException'.
4. ListProjectMemberships 403 -> 404; expected 'User is not permitted to perform operation: ListProjectMemberships', got 'Domain not found'; errortype expected 'AccessDeniedException', got 'ResourceNotFoundException'.
5. GetProject 403 -> 404; expected 'User is not permitted to perform operation: GetProject', got 'Domain not found'; errortype got 'ResourceNotFoundException'.
6. DeleteDomain 403 -> 404; body expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'Domain not found'}; errortype got 'ResourceNotFoundException'.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 and inspect:
- datazone-domain-scoped-denied-sweep-10: task#5 DeleteProject, task#11 DeleteProjectMembership.
- datazone-domain-scoped-denied-sweep-9: task#12 DeleteFormType.
- datazone-domain-scoped-denied-sweep-2: task#9 ListProjectMemberships.
- datazone-domain-scoped-denied-sweep-7: task#3 GetProject.
- datazone-domain-lifecycle: task#5 DeleteDomain.
Each is issued by a caller not permitted on the domain and must answer 403 / AccessDeniedException rather than 404 / ResourceNotFoundException.
