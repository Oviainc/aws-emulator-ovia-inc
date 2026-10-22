Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-cancel-agreement-direct
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-cancel-agreement-direct marketplace-agreement-create-and-accept-agreement-request marketplace-agreement-describe-agreement-malformed-id-rejected marketplace-agreement-search-and-inspect-agreements
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|CancelAgreement|4xx->4xx|$.message
Issue-Key: marketplace-agreement|CancelAgreement|4xx->4xx|status
Issue-Key: marketplace-agreement|AcceptAgreementRequest|4xx->4xx|$.message
Issue-Key: marketplace-agreement|AcceptAgreementRequest|4xx->4xx|status
Issue-Key: marketplace-agreement|DescribeAgreement|4xx->4xx|$.message
Issue-Key: marketplace-agreement|DescribeAgreement|4xx->4xx|status
Issue-Key: marketplace-agreement|GetAgreementTerms|4xx->4xx|$.message
Issue-Key: marketplace-agreement|GetAgreementTerms|4xx->4xx|status
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# Unknown agreement and agreement-request lookups answer 404 ResourceNotFoundException instead of 400 ValidationException

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CancelAgreement

## Description
Four separate marketplace-agreement operations answer a reference to a non-existent resource with an HTTP 404 / ResourceNotFoundException, while the recorded AWS responses answer 400 (the only modeled 400 error on every one of these operations is ValidationException, whose documented role is "The input fails to satisfy the constraints specified by the service"). AWS treats an id that names no agreement on the caller's account as a bad INPUT, not as a missing resource, so the whole error class a client sees is wrong. Four causes, all one fault in how a failed state lookup is mapped to an error; each must be closed.

Cause 1 of 4: CancelAgreement with an agreementId that does not exist.
AWS: Smithy: CancelAgreement.agreementId -> ResourceId (required, length 1..64, pattern ^[A-Za-z0-9_/-]+$); modeled errors include ValidationException (httpError 400, members message/reason/fields) and ResourceNotFoundException (httpError 404). The recording shows status 400 for an unknown agreement id, with message of the form 'Provided agreement does not exist.' (example text; the requirement is the 400 status and the ValidationException code, with reason drawn from ValidationExceptionReason, e.g. INVALID_AGREEMENT_ID).
Emulator: returns status 404 with message 'agreement sim2real-probe-marketplace-agreement-agmt-cd05307f not found'.
Why: a client that branches on error type retries or reports "resource missing" instead of surfacing an input-validation failure; SDK error-shape matching on ValidationException never fires, and code that distinguishes 4xx-validation from 404-not-found (for example deciding whether to re-resolve an id vs. fail the request) takes the wrong branch.
Fix: CancelAgreement's handler / its not-found path in the agreement state lookup — map "no such agreement" to ValidationException(400) with a reason, not to ResourceNotFoundException(404).
Traces: marketplace-agreement-cancel-agreement-direct#task#1.

Cause 2 of 4: AcceptAgreementRequest with an agreementRequestId that does not exist.
AWS: Smithy: AcceptAgreementRequest.agreementRequestId -> AgreementRequestId (required, pattern ^ar-[A-Za-z0-9]+$); errors include ValidationException (400) and ResourceNotFoundException (404). The recording shows status 400 for a well-formed but unknown ar- id, message e.g. 'Provided agreement request does not exist.'
Emulator: returns status 404 with message 'agreement request ar-sim2realprobemarketplaceagreementfake71ebf109 not found'.
Why: same inversion of error class for the accept path, so a caller polling/accepting a request it did not create cannot distinguish AWS's validation rejection from a genuine 404.
Fix: AcceptAgreementRequest's handler / its agreement-request lookup error mapping.
Traces: marketplace-agreement-create-and-accept-agreement-request#task#2.

Cause 3 of 4: DescribeAgreement with a well-formed but unknown agreementId (cluster also covers marketplace-agreement-search-and-inspect-agreements#task#2).
AWS: Smithy: DescribeAgreement.agreementId -> ResourceId (required); errors ValidationException (400) and ResourceNotFoundException (404). The recording shows status 400, message e.g. 'Provided agreement does not exist.'
Emulator: returns status 404 with message 'agreement sim2real-no-such-agreement-7fa92ff8-f1ab-4a21-8241-0845139308ee not found'.
Why: Describe is the read a client uses to decide whether an agreement exists; getting 404 where AWS gives 400 ValidationException flips both the status class and the retry/caching behavior of generated SDK clients.
Fix: DescribeAgreement's handler / state lookup error mapping.
Traces: marketplace-agreement-describe-agreement-malformed-id-rejected#task#2 (and marketplace-agreement-search-and-inspect-agreements#task#2 in the same cluster).

Cause 4 of 4: GetAgreementTerms with an unknown agreementId.
AWS: Smithy: GetAgreementTerms.agreementId -> ResourceId (required); errors ValidationException (400) and ResourceNotFoundException (404). The recording shows status 400, message e.g. 'Provided agreement does not exist.'
Emulator: returns status 404 with message 'agreement ddb0a2ae-b9d0-4c8d-834b-97569f7aeb4d not found'.
Why: terms retrieval for an id the account does not own reports the wrong error class, so pagination/consumer code treats a validation rejection as a deleted resource.
Fix: GetAgreementTerms' handler / state lookup error mapping.
Traces: marketplace-agreement-search-and-inspect-agreements#task#3.

Note: the recorded messages above are quoted as EXAMPLES; the requirements are the 400 status and the ValidationException error code (plus a reason from the modeled enum). No other open issue covers these steps (no open marketplace-agreement issues exist).

## Evidence
Replay of simparity-wire-aws-v1 against the emulator (emulator_up=true, bootstrap_signal=false, state_reset_failures=0):
- marketplace-agreement-cancel-agreement-direct#task#1 CancelAgreement: status expected 400, got 404; $.message expected 'Provided agreement does not exist.', got 'agreement sim2real-probe-marketplace-agreement-agmt-cd05307f not found'.
- marketplace-agreement-create-and-accept-agreement-request#task#2 AcceptAgreementRequest: status expected 400, got 404; $.message expected 'Provided agreement request does not exist.', got 'agreement request ar-sim2realprobemarketplaceagreementfake71ebf109 not found'.
- marketplace-agreement-describe-agreement-malformed-id-rejected#task#2 DescribeAgreement: status expected 400, got 404; $.message expected 'Provided agreement does not exist.', got 'agreement sim2real-no-such-agreement-7fa92ff8-f1ab-4a21-8241-0845139308ee not found' (cluster affected_count=2, also marketplace-agreement-search-and-inspect-agreements#task#2).
- marketplace-agreement-search-and-inspect-agreements#task#3 GetAgreementTerms: status expected 400, got 404; $.message expected 'Provided agreement does not exist.', got 'agreement ddb0a2ae-b9d0-4c8d-834b-97569f7aeb4d not found'.
Model evidence (services/service-com-amazonaws-marketplaceagreement-awsmpcommerceservice-v20200301/smithy-model): ValidationException errorKind=client httpError=400 with members message/reason/fields; ResourceNotFoundException errorKind=client httpError=404; ValidationExceptionReason includes INVALID_AGREEMENT_ID, INVALID_AGREEMENT_REQUEST_ID.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 corpus.
2. Call CancelAgreement with agreementId set to an id that was never created (fixture marketplace-agreement-cancel-agreement-direct, step task#1) -> emulator answers 404, recording expects 400.
3. Call AcceptAgreementRequest with a well-formed but unknown 'ar-...' agreementRequestId (marketplace-agreement-create-and-accept-agreement-request, step task#2) -> 404 instead of 400.
4. Call DescribeAgreement with a well-formed but unknown agreementId (marketplace-agreement-describe-agreement-malformed-id-rejected, step task#2) -> 404 instead of 400.
5. Call GetAgreementTerms with an unknown agreementId (marketplace-agreement-search-and-inspect-agreements, step task#3) -> 404 instead of 400.
6. Re-replay all four fixture steps after the fix; each must return 400 with the ValidationException error code.
