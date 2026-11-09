Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: marketplace-agreement.create-agreement-request-invalid-intent-rejected
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement.create-agreement-request-invalid-intent-rejected|_error_code#6|AssertionError|raise AssertionError(f"expected a ClientError, but the call SUCCEEDED and returned {result!r}")
Issue-Commit: c79cae330c1ee541891fe18bdba894f00759380b

# CreateAgreementRequest accepts an intent outside the Intent enum (NEW/AMEND/REPLACE) and mints an agreement request instead of ValidationException "Provided intent is invalid."

> _Note: this issue was identified against the code base at commit `c79cae330c1ee541891fe18bdba894f00759380b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAgreementRequest

## Description
Cause 1 of 1: CreateAgreementRequest performs no validation of the required `intent` member, so a value that is not a member of the modeled Intent enum is accepted and an agreement request is created.

AWS: Smithy (com.amazonaws.marketplaceagreement, awsmpcommerceservice-v20200301) models CreateAgreementRequestInput.intent as required, target shape `Intent`, an enum whose only values are NEW, AMEND and REPLACE. CreateAgreementRequest models ValidationException (errorKind client, httpError 400, "The input fails to satisfy the constraints specified by the service"). The recorded AWS behavior for an intent outside that enum is HTTP 400 with error code `ValidationException` and message `Provided intent is invalid.` — that message is entirely fixed text for this error class (it names no request value), so it must be reproduced verbatim. The recording further shows that an intent that IS in the enum clears this check and the request is then validated against the referenced proposal, where a well-formed but non-active `agreementProposalIdentifier` (pattern ^(at-|ap-)[A-Za-z0-9]+$) is refused with code `ValidationException` and the different fixed message `Provided agreement proposal is inactive.` — i.e. the intent-enum check is a distinct, earlier refusal and not a blanket failure. Both refusals live in this one handler's input validation; the second assertion was never reached because the first already diverged.

Emulator: the call `CreateAgreementRequest(intent="SUPERSEDE_EVERYTHING", requestedTerms=[{id: <name>}])` returns HTTP 200 with a fully-formed success body — `{"agreementRequestId": "ar-...", "chargeSummary": {"currencyCode": "USD", "newAgreementValue": "0"}}` (the particular id and amount are per-run values, the 200 and the success shape are the divergence). No ValidationException is raised and the unmodeled intent is persisted as new state.

Why: a client that relies on AWS refusing an out-of-enum intent gets the opposite signal. Negative tests that assert the refusal pass vacuously against the emulator; worse, callers that typo or forward an intent from an upstream system see a 200 and an agreementRequestId, so they record a request AWS would never have created, and downstream reads (DescribeAgreementRequest / SearchAgreements) then serve an agreement request carrying an intent value no AWS consumer can interpret. It also hides the real error-class distinction: against AWS the enum rejection and the proposal-inactive rejection are two different messages, and the emulator surfaces neither.

Fix: the CreateAgreementRequest handler's request validation in the marketplace-agreement service crate — validate `intent` against the modeled Intent enum (NEW/AMEND/REPLACE) before any state mutation and return the modeled ValidationException (HTTP 400, code ValidationException, message `Provided intent is invalid.`); the proposal-reachability check that yields `Provided agreement proposal is inactive.` belongs after it in the same validation path, so that a valid intent with a non-active proposal gets that message rather than a success.

Traces: behavioral check marketplace-agreement.create-agreement-request-invalid-intent-rejected (group overly-permissive, axis:rejection, cap:c1) fails at its first assertion with "expected a ClientError, but the call SUCCEEDED and returned {'agreementRequestId': 'ar-0000000000000001', ...}".

## Evidence
Expected (real AWS, per the recorded check): CreateAgreementRequest with intent="SUPERSEDE_EVERYTHING" is refused — HTTP 400, Error.Code = "ValidationException", Error.Message = "Provided intent is invalid." (fixed text, no variable parts). Control: intent="NEW" with agreementProposalIdentifier="ap-<nonexistent>" is also refused, but with Error.Message = "Provided agreement proposal is inactive."

Actual (emulator): HTTP 200, body {'agreementRequestId': 'ar-0000000000000001', 'chargeSummary': {'currencyCode': 'USD', 'newAgreementValue': '0'}}, content-type application/x-amz-json-1.0. No ClientError raised. Harness assertion: AssertionError: expected a ClientError, but the call SUCCEEDED and returned {...} (check_marketplace_agreement_create_agreement_request_invalid_intent_rejected.py line 53).

Smithy citation: CreateAgreementRequestInput.intent -> Intent (enum, values [NEW, AMEND, REPLACE], required); CreateAgreementRequest errors include ValidationException (client, httpError 400).

## Steps to Reproduce
1. Point a botocore client at the emulator: client = boto3.client("marketplace-agreement", region_name="us-east-1", endpoint_url=<emulator>) (the service answers only in us-east-1).
2. Call client.create_agreement_request(intent="SUPERSEDE_EVERYTHING", requestedTerms=[{"id": "sim2real-term-x"}]).
3. Observe HTTP 200 and an agreementRequestId in the response. Real AWS raises ClientError with Code="ValidationException" and Message="Provided intent is invalid."
4. Control, to show the refusal is enum-specific and not blanket: call client.create_agreement_request(intent="NEW", requestedTerms=[{"id": "sim2real-term-x"}], agreementProposalIdentifier="ap-sim2realnonexistentproposal") and observe it is not refused with ValidationException / "Provided agreement proposal is inactive." either.
5. Replay: behavioral check marketplace-agreement.create-agreement-request-invalid-intent-rejected.
