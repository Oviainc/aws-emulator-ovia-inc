Issue-Source: trace-sweep-divergence
Issue-Trace: datazone-domain-lifecycle
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: datazone-domain-lifecycle
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|CreateDomain|4xx->2xx|$.message
Issue-Key: datazone|CreateDomain|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: datazone|CreateDomain|4xx->2xx|status
Issue-Key: datazone|CreateDomain|4xx->4xx|$.message
Issue-Key: datazone|CreateDomain|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: datazone|CreateDomain|4xx->4xx|status
Issue-Key: datazone|ListDomains|2xx->2xx|$.domains
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# datazone domain lifecycle: CreateDomain skips required-member and role checks, ListDomains omits its empty list

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDomain

## Description
THREE causes in the domain lifecycle: the emulator accepts a CreateDomain request datazone rejects, mis-classifies a role failure, and omits a list member from a successful ListDomains response. Fix all three before submitting.

Cause 1 of 3: CreateDomain accepts a request with no domainExecutionRole and creates the domain.
AWS: Smithy CreateDomain POST /v2/domains (success code 201), errors include ValidationException (httpError 400, message @required). The model does not mark domainExecutionRole @required (recorded behavior only for the requirement itself), but the recorded response shows datazone rejecting the request service-side: 400 ValidationException with message `1 validation error detected: Value null at 'domainExecutionRole' failed to satisfy constraint: Member must not be null` — fixed text "1 validation error detected: Value null at '" ... "' failed to satisfy constraint: Member must not be null", with the member name as the only variable part — and an x-amzn-errortype header on the response.
Emulator: 201 Created with a domain body; no message member and no x-amzn-errortype header at all.
Why: the rejected request appears to succeed, so a client believes it holds a usable domain, and every later step in a provisioning flow runs against state AWS would never have created (see cause 2). Any test that asserts the required-member failure passes against AWS and fails against the emulator.
Fix: CreateDomain's input validation (service-side required-member check for domainExecutionRole, emitting the standard "Member must not be null" ValidationException) before any state mutation.
Traces: datazone-domain-lifecycle#task#1.

Cause 2 of 3: CreateDomain with an execution role it cannot assume returns 409 ConflictException instead of 403.
AWS: Smithy CreateDomain binds AccessDeniedException (httpError 403) and ConflictException (httpError 409); domainExecutionRole is a RoleArn with pattern "^arn:aws[^:]*:iam::\\d{12}:role(/[a-zA-Z0-9+=,.@_-]+)*/[a-zA-Z0-9+=,.@_-]+$". When the named role cannot be assumed, the recorded response is 403 with message `Unable to assume role '<domainExecutionRole>'` — fixed text "Unable to assume role '" ... "'" with the role ARN taken verbatim from the request (the recording's arn:aws:iam::000000000000:role/... is just this run's value).
Emulator: 409 with x-amzn-errortype ConflictException and message 'Domain name already exists under this account, please use another one' — it never checks the role and instead reports a name collision. Hypothesis: the collision exists only because cause 1 wrongly created a domain with this name in the preceding step; the missing assume-role check is the defect either way.
Why: a caller sees a retryable-looking name conflict and renames the domain, when the real problem is an unusable execution role; 409 vs 403 also sends SDK error handling down the wrong branch.
Fix: CreateDomain handler — validate the execution role is assumable and raise AccessDeniedException (403) with AWS's message, and only report ConflictException for a genuine name collision.
Traces: datazone-domain-lifecycle#task#2.

Cause 3 of 3: ListDomains omits the domain list when it is empty.
AWS: Smithy ListDomains GET /v2/domains, 200; ListDomainsOutput's domain-summary list member is @required (model name `items`, target DomainSummaries), and the recorded response serialises it as an empty array (recorded key `domains`) rather than omitting it. A required list is always present on the wire, empty or not.
Emulator: 200 with the list member absent from the body entirely.
Why: SDKs and clients that read the required list get null/undefined instead of an empty collection, so iteration or paging over "no domains" throws instead of yielding zero items.
Fix: ListDomains' response serializer/state projection — always emit the domain list member, as [] when there are no domains.
Traces: datazone-domain-lifecycle#task#6.

## Evidence
Replay report, expected -> actual:
1. CreateDomain task#1: status expected 400, got 201; $.message expected "1 validation error detected: Value null at 'domainExecutionRole' failed to satisfy constraint: Member must not be null", got None; header x-amzn-errortype expected (shaped, not value-asserted), got None.
2. CreateDomain task#2: status expected 403, got 409; $.message expected "Unable to assume role 'arn:aws:iam::000000000000:role/sim2real-dz-absent-exec-role'", got 'Domain name already exists under this account, please use another one'; x-amzn-errortype got 'ConflictException'.
3. ListDomains task#6: status 200 on both sides; $.domains expected [], got None (member absent from the response body).

## Steps to Reproduce
Replay trace set simparity-wire-aws-v1, fixture datazone-domain-lifecycle:
1. task#1 — CreateDomain with domainExecutionRole omitted/null: expect 400 ValidationException with the "Value null at 'domainExecutionRole' ... Member must not be null" message and no domain created; the emulator returns 201.
2. task#2 — CreateDomain naming an execution role that cannot be assumed: expect 403 with "Unable to assume role '<roleArn>'"; the emulator returns 409 ConflictException about a duplicate domain name.
3. task#6 — ListDomains with no domains present: expect 200 with the domain list member serialised as an empty array; the emulator omits the member.
