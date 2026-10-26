Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: dc6b9d697210888fba6fd46cd7393ea22255dcd4

# Cancelling a non-active agreement answers 409 ConflictException instead of 400 ValidationException with reason INACTIVE_AGREEMENT

_Filed: 2026-10-24T12:48:09Z_
_Operation: CancelAgreement | Class: error-response_

> _Note: this issue was identified against the code base at commit `dc6b9d697210888fba6fd46cd7393ea22255dcd4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

CancelAgreement is "allows an acceptor to cancel an active agreement. Not all agreements are eligible for cancellation. Use the error response to determine why a cancellation request was rejected." The service's ValidationExceptionReason enum carries INACTIVE_AGREEMENT precisely for "the agreement named is not active", and this emulator already uses it for exactly that condition on a sibling operation (src/ops/send_agreement_cancellation_request.rs refuses a non-ACTIVE agreement with ValidationException/INACTIVE_AGREEMENT on the agreementId member). Expected: CancelAgreement on an agreement that is CANCELLED/REPLACED/EXPIRED is refused with 400 ValidationException (reason INACTIVE_AGREEMENT, field agreementId), i.e. the input-fault classification the reason enum defines.

Actual: src/ops/cancel_agreement.rs returns ConflictException (HTTP 409) for every non-ACTIVE status, so a client that branches on ValidationException/reason to learn WHY the cancellation was refused gets an error class with no reason member at all, and the two paths that refuse the same condition in this service disagree on status code.

## Evidence

Spec: CancelAgreement operation doc ("an active agreement... Use the error response to determine why a cancellation request was rejected"); ValidationExceptionReason includes INACTIVE_AGREEMENT; ValidationException is modelled on CancelAgreement (HTTP 400) and carries reason + fields, which ConflictException does not. Internal inconsistency: src/ops/send_agreement_cancellation_request.rs lines ~230-237 refuse the identical "agreement is not ACTIVE" condition with validation_exception("INACTIVE_AGREEMENT", "agreementId"), while src/ops/cancel_agreement.rs lines ~165-179 raise ConflictException. Test coverage: tests/all/agreement_lifecycle.rs asserts the current 409 for a second cancel, so the behavior is pinned and would need updating.

## Steps to Reproduce

1. CreateAgreementRequest + AcceptAgreementRequest to mint an ACTIVE agreement
2. CancelAgreement(agreementId=<id>) -> 200 (the agreement becomes CANCELLED)
3. CancelAgreement(agreementId=<id>) again -> observe 409 ConflictException (expected 400 ValidationException with reason INACTIVE_AGREEMENT and fields naming agreementId)
4. Alternatively accept a REPLACE agreement request so the source agreement becomes REPLACED, then CancelAgreement on the source -> observe the same 409
