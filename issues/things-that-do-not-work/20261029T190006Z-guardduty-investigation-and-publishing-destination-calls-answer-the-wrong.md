Issue-Source: trace-sweep-divergence
Issue-Trace: guardduty-investigation-lifecycle
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: guardduty-investigation-lifecycle guardduty-tagging-created-with-tags guardduty-tagging-round-trip guardduty-investigation-not-found guardduty-investigation-workflow guardduty-publishing-destination-error-paths guardduty-publishing-destination-not-found guardduty-publishing-destination-setup-attempt
Issue-Shape-Id: com.amazonaws.guardduty#GuardDutyAPIService
Issue-Key: guardduty|GetInvestigation|4xx->4xx|$.message
Issue-Key: guardduty|CreateInvestigation|4xx->2xx|$.__type
Issue-Key: guardduty|CreateInvestigation|4xx->2xx|$.message
Issue-Key: guardduty|CreateInvestigation|4xx->2xx|$.type
Issue-Key: guardduty|CreateInvestigation|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|CreateInvestigation|4xx->2xx|status
Issue-Key: guardduty|CreateInvestigation|4xx->4xx|$.__type
Issue-Key: guardduty|CreateInvestigation|4xx->4xx|$.message
Issue-Key: guardduty|CreateInvestigation|4xx->4xx|$.type
Issue-Key: guardduty|CreateInvestigation|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: guardduty|CreateInvestigation|4xx->4xx|status
Issue-Key: guardduty|DescribePublishingDestination|4xx->4xx|$.message
Issue-Key: guardduty|DeletePublishingDestination|4xx->4xx|$.message
Issue-Key: guardduty|CreatePublishingDestination|4xx->2xx|$.__type
Issue-Key: guardduty|CreatePublishingDestination|4xx->2xx|$.message
Issue-Key: guardduty|CreatePublishingDestination|4xx->2xx|$.type
Issue-Key: guardduty|CreatePublishingDestination|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: guardduty|CreatePublishingDestination|4xx->2xx|status
Issue-Key: guardduty|CreatePublishingDestination|4xx->4xx|$.message
Issue-Related: 20261029T190006Z-guardduty-read-responses-omit-required-scalars-and-invent-empty.md
Issue-Commit: 611eef386f1100e1b77f5a1515bab52b7a7a6aeb

# Investigation and publishing-destination calls answer the wrong status

> _Note: this issue was identified against the code base at commit `611eef386f1100e1b77f5a1515bab52b7a7a6aeb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInvestigation

## Description
CreateInvestigation answers the wrong success status and has neither of the two gates GuardDuty applies to it (the AI_ANALYST feature prerequisite and the detector-ownership check), and the publishing-destination operations refuse with emulator-authored messages or do not refuse at all. 8 causes.

Cause 1 of 8: CreateInvestigation answers 202 where AWS answers 200.
AWS: the recorded successful CreateInvestigation response status is 200. (Smithy declares @http code 202 on the operation, so the emulator is following the model; the recording shows the service does not, which is why this is a wire divergence and the recorded status is authoritative here.)
Emulator: 202.
Why: a client that asserts on an exact status code — or a gateway/retry layer configured for 200 on this call — treats a successful investigation creation as an unexpected response.
Fix: the CreateInvestigation handler's success status.
Traces: guardduty-investigation-lifecycle#task#4.

Cause 2 of 8: GetInvestigation's not-found message.
AWS: HTTP 404 with message 'Resource not found.' — a short fixed sentence naming neither the operation nor the id.
Emulator: 'The request is rejected because the requested investigation could not be found.'
Why: status and class are right, so the message is the only observable; a caller that surfaces it shows text real GuardDuty never emits.
Fix: the GetInvestigation handler's ResourceNotFoundException message.
Traces: guardduty-investigation-not-found#task#2.

Cause 3 of 8: CreateInvestigation has no AI_ANALYST feature prerequisite.
AWS: HTTP 400, x-amzn-errortype 'BadRequestException', $.__type/$.type 'InvalidInputException', message "The request is rejected because the AI Analyst feature is not enabled for the delegated administrator account. Enable the AI_ANALYST feature on your delegated administrator's detector to use the Investigation API." — entirely fixed text.
Emulator: HTTP 202 — it creates the investigation, with no error members at all.
Why: the Investigation API's one prerequisite is unenforced, so an investigation is created and recorded for a detector that has not enabled AI_ANALYST; a client's feature-gating path is never exercised and an emulator-backed test of it passes vacuously.
Fix: a feature-enablement check in the CreateInvestigation handler, before the investigation is written to state.
Traces: guardduty-investigation-workflow#task#2.

Cause 4 of 8: CreateInvestigation refuses a foreign detectorId with 403 instead of 400.
AWS: HTTP 400, x-amzn-errortype 'BadRequestException', $.__type/$.type 'InvalidInputException', message 'The request is rejected because the input detectorId is not owned by the current account.' — fixed text.
Emulator: HTTP 403 with AccessDeniedException in the header and in both body fields, message 'The request is rejected because the current account is not authorized to perform this operation on the input detectorId.'
Why: the SDK raises AccessDeniedException where the caller expects BadRequestException, so retry/permission-escalation logic keyed on a 403 fires on what AWS classifies as a malformed input. Smithy models both AccessDeniedException(403) and BadRequestException(400) on CreateInvestigation, so the handler is choosing the wrong one.
Fix: the CreateInvestigation handler's detector-ownership branch and the error it maps to.
Traces: guardduty-investigation-workflow#task#3.

Cause 5 of 8: DescribePublishingDestination's refusal message for an unusable destinationId.
AWS: HTTP 400, InvalidInputException, message 'The request is rejected because the one or more input parameters have invalid values.' — fixed text, naming no member.
Emulator: 'The request is rejected because the parameter destinationId has an invalid value.'
Why: the emulator names the member AWS deliberately leaves out, so a client matching the documented sentence does not match.
Fix: the DescribePublishingDestination handler's refusal message.
Traces: guardduty-publishing-destination-error-paths#task#2.

Cause 6 of 8: DeletePublishingDestination's refusal message for an unusable destinationId.
AWS: HTTP 400, InvalidInputException, 'The request is rejected because the one or more input parameters have invalid values.'
Emulator: 'The request is rejected because the parameter destinationId has an invalid value.'
Why: as cause 5.
Fix: the DeletePublishingDestination handler's refusal message.
Traces: guardduty-publishing-destination-error-paths#task#3.

Cause 7 of 8: CreatePublishingDestination does not check that the destinationArn resolves.
AWS: HTTP 400, x-amzn-errortype 'BadRequestException', $.__type/$.type 'InvalidInputException', message 'The request failed because the resource specified in the destinationArn parameter does not exist.' — fixed text.
Emulator: HTTP 200 — it creates the destination, with no error members at all.
Why: a publishing destination is recorded pointing at a resource that does not exist, so findings are configured to publish nowhere and the failure surfaces only much later (or never) instead of at configuration time.
Fix: destination-resource existence validation in the CreatePublishingDestination handler, before the destination is written to state.
Traces: guardduty-publishing-destination-error-paths#task#4.

Cause 8 of 8: CreatePublishingDestination misreports an invalid kmsKeyArn as a missing one.
AWS: HTTP 400, InvalidInputException, message 'The request failed because the value specified for the kmsKeyArn parameter is not valid.' — the value was supplied and is unusable.
Emulator: 'The request is rejected because the parameter destinationProperties.kmsKeyArn is required.'
Why: the caller is told it omitted a parameter it actually supplied, so the obvious remediation (add the key) is already done and the real fault — the key ARN is not valid — is never surfaced.
Fix: the CreatePublishingDestination handler's kmsKeyArn validation: distinguish absent from invalid and report AWS's not-valid text.
Traces: guardduty-publishing-destination-setup-attempt#task#3.

Related open issue (does NOT cover these; it carries no replay key for them): 20261028T224259Z-guardduty-deletepublishingdestination-wrong-behavior.md.

## Evidence
Cause 1: status expected 200, got 202 (no body divergence).
Cause 2: $.message expected 'Resource not found.', got 'The request is rejected because the requested investigation could not be found.' (404 both sides)
Cause 3: status expected 400, got 202; $.message expected "The request is rejected because the AI Analyst feature is not enabled for the delegated administrator account. Enable the AI_ANALYST feature on your delegated administrator's detector to use the Investigation API.", $.__type/$.type expected 'InvalidInputException', x-amzn-errortype expected 'BadRequestException' — all absent.
Cause 4: status expected 400, got 403; x-amzn-errortype and $.__type/$.type expected 'BadRequestException'/'InvalidInputException', got 'AccessDeniedException'; $.message expected 'The request is rejected because the input detectorId is not owned by the current account.', got 'The request is rejected because the current account is not authorized to perform this operation on the input detectorId.'
Causes 5-6: $.message expected 'The request is rejected because the one or more input parameters have invalid values.', got 'The request is rejected because the parameter destinationId has an invalid value.'
Cause 7: status expected 400, got 200; $.message expected 'The request failed because the resource specified in the destinationArn parameter does not exist.' and the error members expected present — all absent.
Cause 8: $.message expected 'The request failed because the value specified for the kmsKeyArn parameter is not valid.', got 'The request is rejected because the parameter destinationProperties.kmsKeyArn is required.'

## Steps to Reproduce
1. Replay guardduty-investigation-lifecycle step task#4: a successful CreateInvestigation answers 202 instead of 200.
2. Replay guardduty-investigation-workflow step task#2 (detector without AI_ANALYST: 202 instead of 400) and step task#3 (foreign detectorId: 403/AccessDeniedException instead of 400/BadRequestException).
3. Replay guardduty-investigation-not-found step task#2 and compare $.message.
4. Replay guardduty-publishing-destination-error-paths steps task#2, task#3 and task#4 (the last creates a destination for a nonexistent destinationArn and returns 200), and guardduty-publishing-destination-setup-attempt step task#3 for the kmsKeyArn message.
