Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-domain-scoped-denied-sweep-5
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-domain-scoped-denied-sweep-5 datazone-domain-scoped-denied-sweep-6 datazone-domain-scoped-denied-sweep-3 datazone-domain-lifecycle
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|GetAccountPool|4xx->4xx|$.message
Issue-Key: datazone|GetAccountPool|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetAccountPool|4xx->4xx|status
Issue-Key: datazone|GetAsset|4xx->4xx|$.message
Issue-Key: datazone|GetAsset|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetAsset|4xx->4xx|status
Issue-Key: datazone|GetAssetFilter|4xx->4xx|$.message
Issue-Key: datazone|GetAssetFilter|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetAssetFilter|4xx->4xx|status
Issue-Key: datazone|GetAssetType|4xx->4xx|$.message
Issue-Key: datazone|GetAssetType|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetAssetType|4xx->4xx|status
Issue-Key: datazone|GetDomainUnit|4xx->4xx|$.message
Issue-Key: datazone|GetDomainUnit|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetDomainUnit|4xx->4xx|status
Issue-Key: datazone|GetFormType|4xx->4xx|$.message
Issue-Key: datazone|GetFormType|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetFormType|4xx->4xx|status
Issue-Key: datazone|GetGlossary|4xx->4xx|$.message
Issue-Key: datazone|GetGlossary|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetGlossary|4xx->4xx|status
Issue-Key: datazone|GetGlossaryTerm|4xx->4xx|$.message
Issue-Key: datazone|GetGlossaryTerm|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetGlossaryTerm|4xx->4xx|status
Issue-Key: datazone|GetListing|4xx->4xx|$.message
Issue-Key: datazone|GetListing|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetListing|4xx->4xx|status
Issue-Key: datazone|GetDomain|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|GetDomain|4xx->4xx|status
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# datazone domain-scoped read operations answer 404 ResourceNotFoundException instead of 403 AccessDeniedException

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAccountPool

## Description
TEN causes, one shared fault: for a single-resource read inside a domain the caller is not authorized on, datazone denies the request BEFORE any resource lookup, so authorization — not existence — decides the answer. The emulator instead performs the lookup first and reports the resulting miss, turning a 403 AccessDeniedException into a 404 ResourceNotFoundException. The ordering belongs in the shared domain-scoped authorization pre-check that runs ahead of every operation's state lookup; each operation then only needs its own operation name in the message. Fix all ten before submitting.

Shared AWS behavior (Smithy): every one of these operations binds AccessDeniedException (httpError 403, doc "You do not have sufficient access to perform this action.", message @required) and ResourceNotFoundException (httpError 404). The recorded denial is 403 with x-amzn-errortype AccessDeniedException and message `User is not permitted to perform operation: <OperationName>` — fixed text "User is not permitted to perform operation: " plus the operation name of the request; nothing from the request path appears in it. Shared emulator behavior: 404 with x-amzn-errortype ResourceNotFoundException and an existence message. Shared Why: a client cannot distinguish "not allowed" from "does not exist" — retry/provisioning logic treats a permissions failure as a missing resource and creates or re-creates state, and SDK error-type branching on AccessDeniedException never fires. Shared Fix: datazone's request authorization layer (deny before resource resolution) plus the AccessDeniedException message construction.

Cause 1 of 10: GetAccountPool. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetAccountPool`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-5#task#1.

Cause 2 of 10: GetAsset. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetAsset` (Smithy GetAsset GET /v2/domains/{domainIdentifier}/assets/{identifier}, errors include AccessDeniedException 403 and ResourceNotFoundException 404). Emulator: 404 ResourceNotFoundException, message 'Domain <domainIdentifier> was not found'. Traces: datazone-domain-scoped-denied-sweep-5#task#2.

Cause 3 of 10: GetAssetFilter. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetAssetFilter`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-5#task#3.

Cause 4 of 10: GetAssetType. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetAssetType`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-5#task#4.

Cause 5 of 10: GetDomainUnit. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetDomainUnit`. Emulator: 404 ResourceNotFoundException, message 'Domain unit not found' — here the emulator even resolves past the domain to the child resource before failing. Traces: datazone-domain-scoped-denied-sweep-5#task#7.

Cause 6 of 10: GetFormType. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetFormType`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-6#task#3.

Cause 7 of 10: GetGlossary. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetGlossary`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-6#task#4.

Cause 8 of 10: GetGlossaryTerm. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetGlossaryTerm`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-6#task#5.

Cause 9 of 10: GetListing. AWS: 403 AccessDeniedException, message `User is not permitted to perform operation: GetListing`. Emulator: 404 ResourceNotFoundException, message 'Domain not found'. Traces: datazone-domain-scoped-denied-sweep-3#task#6.

Cause 10 of 10: GetDomain. AWS: 403 with x-amzn-errortype AccessDeniedException (the recording asserts status and error type; the message was not asserted, so only the status/type are requirements here). Emulator: 404 with x-amzn-errortype ResourceNotFoundException. Traces: datazone-domain-lifecycle#task#3.

## Evidence
Replay report, expected -> actual:
1. GetAccountPool 403 -> 404; $.message expected 'User is not permitted to perform operation: GetAccountPool', got 'Domain not found'; x-amzn-errortype expected 'AccessDeniedException', got 'ResourceNotFoundException'.
2. GetAsset 403 -> 404; expected 'User is not permitted to perform operation: GetAsset', got 'Domain dzd_bogus00000000001 was not found'; errortype AccessDeniedException -> ResourceNotFoundException.
3. GetAssetFilter 403 -> 404; expected 'User is not permitted to perform operation: GetAssetFilter', got 'Domain not found'; errortype AccessDeniedException -> ResourceNotFoundException.
4. GetAssetType 403 -> 404; expected 'User is not permitted to perform operation: GetAssetType', got 'Domain not found'; errortype AccessDeniedException -> ResourceNotFoundException.
5. GetDomainUnit 403 -> 404; expected 'User is not permitted to perform operation: GetDomainUnit', got 'Domain unit not found'; errortype AccessDeniedException -> ResourceNotFoundException.
6. GetFormType 403 -> 404; expected 'User is not permitted to perform operation: GetFormType', got 'Domain not found'; errortype got 'ResourceNotFoundException'.
7. GetGlossary 403 -> 404; expected 'User is not permitted to perform operation: GetGlossary', got 'Domain not found'.
8. GetGlossaryTerm 403 -> 404; expected 'User is not permitted to perform operation: GetGlossaryTerm', got 'Domain not found'.
9. GetListing 403 -> 404; expected 'User is not permitted to perform operation: GetListing', got 'Domain not found'; errortype AccessDeniedException -> ResourceNotFoundException.
10. GetDomain 403 -> 404; x-amzn-errortype got 'ResourceNotFoundException' where AccessDeniedException was recorded.

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1 and inspect these steps, each a read issued by a caller that is not permitted on the target domain:
- datazone-domain-scoped-denied-sweep-5: task#1 GetAccountPool, task#2 GetAsset, task#3 GetAssetFilter, task#4 GetAssetType, task#7 GetDomainUnit.
- datazone-domain-scoped-denied-sweep-6: task#3 GetFormType, task#4 GetGlossary, task#5 GetGlossaryTerm.
- datazone-domain-scoped-denied-sweep-3: task#6 GetListing.
- datazone-domain-lifecycle: task#3 GetDomain.
Each must answer 403 with x-amzn-errortype AccessDeniedException and the "User is not permitted to perform operation: <Operation>" message instead of a 404 existence error.
