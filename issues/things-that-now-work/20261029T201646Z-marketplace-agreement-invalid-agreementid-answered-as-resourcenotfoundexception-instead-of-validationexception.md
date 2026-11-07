Issue-Source: trace-sweep-divergence
Issue-Trace: marketplace-agreement-agreement-payment-request-workflow
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: marketplace-agreement-agreement-payment-request-workflow
Issue-Shape-Id: com.amazonaws.marketplaceagreement#AWSMPCommerceService_v20200301
Issue-Key: marketplace-agreement|GetAgreementPaymentRequest|4xx->4xx|$.__type
Issue-Key: marketplace-agreement|GetAgreementPaymentRequest|4xx->4xx|$.fields
Issue-Key: marketplace-agreement|GetAgreementPaymentRequest|4xx->4xx|$.message
Issue-Key: marketplace-agreement|GetAgreementPaymentRequest|4xx->4xx|$.reason
Issue-Key: marketplace-agreement|AcceptAgreementPaymentRequest|4xx->4xx|$.__type
Issue-Key: marketplace-agreement|AcceptAgreementPaymentRequest|4xx->4xx|$.fields
Issue-Key: marketplace-agreement|AcceptAgreementPaymentRequest|4xx->4xx|$.message
Issue-Key: marketplace-agreement|AcceptAgreementPaymentRequest|4xx->4xx|$.reason
Issue-Key: marketplace-agreement|RejectAgreementPaymentRequest|4xx->4xx|$.__type
Issue-Key: marketplace-agreement|RejectAgreementPaymentRequest|4xx->4xx|$.fields
Issue-Key: marketplace-agreement|RejectAgreementPaymentRequest|4xx->4xx|$.message
Issue-Key: marketplace-agreement|RejectAgreementPaymentRequest|4xx->4xx|$.reason
Issue-Key: marketplace-agreement|CancelAgreementPaymentRequest|4xx->4xx|$.__type
Issue-Key: marketplace-agreement|CancelAgreementPaymentRequest|4xx->4xx|$.fields
Issue-Key: marketplace-agreement|CancelAgreementPaymentRequest|4xx->4xx|$.message
Issue-Key: marketplace-agreement|CancelAgreementPaymentRequest|4xx->4xx|$.reason
Issue-Commit: 0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f

# Invalid agreementId answered as ResourceNotFoundException instead of ValidationException INVALID_AGREEMENT_ID

> _Note: this issue was identified against the code base at commit `0d251cb15d5c6a3e8a766cb6c72620a30f43ef7f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAgreementPaymentRequest

## Description
Every payment-request operation that takes an `agreementId` classifies a syntactically invalid agreement identifier as a missing resource instead of as an input-validation failure. The four causes below are the same defect reached through four handlers; each must be closed before this issue is submitted.

Cause 1 of 4: GetAgreementPaymentRequest with an invalid agreementId.
AWS: `agreementId` is a required member targeting `AgreementId` with @length min 1 / max 64 and @pattern ^[A-Za-z0-9_/-]+$ (Smithy: GetAgreementPaymentRequest.agreementId). A value that cannot be a well-formed agreement identifier fails the input constraint, so the service answers HTTP 400 with the service-common `ValidationException` (Smithy: ValidationException @error client, @httpError 400) carrying `__type` `com.amazonaws.mp.commerce.v20200301#ValidationException`, `reason` = `INVALID_AGREEMENT_ID` (a modeled `ValidationExceptionReason` value), `message` = the fixed text `Provided agreement id is invalid.`, and a `fields` list with one `ValidationExceptionField` whose `name` is the offending input member path (here `agreementId`, taken from the request) and whose `message` is the fixed text `Member is invalid`. Only a well-formed id that names no stored agreement is a `ResourceNotFoundException`, and that error is modeled @httpError 404, not 400.
Emulator: answers HTTP 400 with `__type` `ResourceNotFoundException` and `message` `Provided agreement does not exist.`, and omits `reason` and `fields` entirely. It is wrong twice over: the wrong error class for the input, and a 400 status for an error the model binds to 404.
Why: a client that branches on the error code sees a not-found and concludes the agreement was deleted or belongs to another account, so it stops retrying or re-resolves the resource, when in fact its own request was malformed. Clients and SDK retry/validation layers that read `reason` and `fields` to report which member was rejected get nothing to report, because both members are absent.
Fix: the shared agreement-id resolution path used by the payment-request handlers must validate the identifier's shape against the `AgreementId` constraints before it looks the agreement up in state, and emit `ValidationException` (reason `INVALID_AGREEMENT_ID`, one `fields` entry naming `agreementId`) for a value that fails the pattern/length; the not-found path stays for well-formed ids only, and must serialize at its modeled 404.
Traces: marketplace-agreement-agreement-payment-request-workflow#task#2.

Cause 2 of 4: AcceptAgreementPaymentRequest with an invalid agreementId.
AWS: same required `agreementId` constraint and the same `ValidationException` body as Cause 1 — status 400, `__type` `com.amazonaws.mp.commerce.v20200301#ValidationException`, `reason` `INVALID_AGREEMENT_ID`, `message` `Provided agreement id is invalid.`, `fields` = [{name: `agreementId`, message: `Member is invalid`}].
Emulator: 400 with `__type` `ResourceNotFoundException`, `message` `Provided agreement does not exist.`, no `reason`, no `fields`.
Why: as Cause 1 — an acceptor retrying against a corrected id is told the agreement does not exist, which is not recoverable advice.
Fix: AcceptAgreementPaymentRequest's input validation, via the same shared agreement-id check.
Traces: marketplace-agreement-agreement-payment-request-workflow#task#4.

Cause 3 of 4: RejectAgreementPaymentRequest with an invalid agreementId.
AWS: identical to Cause 1 — 400 `ValidationException`, `reason` `INVALID_AGREEMENT_ID`, `message` `Provided agreement id is invalid.`, `fields` = [{name: `agreementId`, message: `Member is invalid`}].
Emulator: 400 `ResourceNotFoundException` / `Provided agreement does not exist.`, `reason` and `fields` missing.
Why: as Cause 1.
Fix: RejectAgreementPaymentRequest's input validation, via the same shared agreement-id check.
Traces: marketplace-agreement-agreement-payment-request-workflow#task#5.

Cause 4 of 4: CancelAgreementPaymentRequest with an invalid agreementId.
AWS: identical to Cause 1 — 400 `ValidationException`, `reason` `INVALID_AGREEMENT_ID`, `message` `Provided agreement id is invalid.`, `fields` = [{name: `agreementId`, message: `Member is invalid`}].
Emulator: 400 `ResourceNotFoundException` / `Provided agreement does not exist.`, `reason` and `fields` missing.
Why: as Cause 1.
Fix: CancelAgreementPaymentRequest's input validation, via the same shared agreement-id check.
Traces: marketplace-agreement-agreement-payment-request-workflow#task#6.

No open issue carries these replay keys. The nearest open issues (20261022T212948Z-marketplace-agreement-intent-enum-and-proposal-validation-skipped.md, 20261029T014620Z-marketplace-agreement-batchcreatebillingadjustmentrequest-error-response.md) concern CreateAgreementRequest/BatchCreateBillingAdjustmentRequest validation, not the agreement-id classification in the payment-request handlers.

## Evidence
Replay of trace set simparity-wire-aws-v1, fixture marketplace-agreement-agreement-payment-request-workflow (verdict mismatch, expected 400 / actual 400 on all four steps):

step task#2 GetAgreementPaymentRequest:
  $.__type: expected 'com.amazonaws.mp.commerce.v20200301#ValidationException', got 'ResourceNotFoundException'
  $.message: expected 'Provided agreement id is invalid.', got 'Provided agreement does not exist.'
  $.reason: expected 'INVALID_AGREEMENT_ID', got None (member absent)
  $.fields: expected [{'name': 'agreementId', 'message': 'Member is invalid'}], got None (member absent)

step task#4 AcceptAgreementPaymentRequest: identical four-key divergence.
step task#5 RejectAgreementPaymentRequest: identical four-key divergence.
step task#6 CancelAgreementPaymentRequest: identical four-key divergence.

Fixed vs variable text: `Provided agreement id is invalid.` and the field message `Member is invalid` are fixed texts of this error class; the `fields[].name` value (`agreementId`) is the rejected input member path taken from the request. No per-run value (request id, timestamp) is involved in any of the four divergences.

Model support: ValidationException @error client @httpError 400 with members message/reason/fields; ValidationExceptionReason includes INVALID_AGREEMENT_ID; ValidationExceptionField requires name and message; ResourceNotFoundException @httpError 404.

## Steps to Reproduce
1. Boot the emulator from the mainline checkout.
2. Replay fixture marketplace-agreement-agreement-payment-request-workflow, step task#2 (GetAgreementPaymentRequest): call the operation with a syntactically invalid agreementId (a value that violates AgreementId @pattern ^[A-Za-z0-9_/-]+$ / @length 1..64) and a valid paymentRequestId.
3. Observe HTTP 400 with __type ResourceNotFoundException and message 'Provided agreement does not exist.', with no reason and no fields members, instead of __type com.amazonaws.mp.commerce.v20200301#ValidationException with reason INVALID_AGREEMENT_ID, message 'Provided agreement id is invalid.' and fields [{name: agreementId, message: 'Member is invalid'}].
4. Repeat with steps task#4 (AcceptAgreementPaymentRequest), task#5 (RejectAgreementPaymentRequest) and task#6 (CancelAgreementPaymentRequest) of the same fixture: each reproduces the same substitution.
