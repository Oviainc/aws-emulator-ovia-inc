Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.create-workflow-rejects-unknown-step-type-and-targetless-custom
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.create-workflow-rejects-unknown-step-type-and-targetless-custom|run#12|AssertionError|assert msg_target == 'Invalid function provided', (
Issue-Commit: cdc6bb70501df7dfce64380a94b57487d7a7ceff

# CreateWorkflow rejects a targetless CUSTOM step with an emulator-authored message instead of AWS's 'Invalid function provided'

> _Note: this issue was identified against the code base at commit `cdc6bb70501df7dfce64380a94b57487d7a7ceff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
One net-new root cause in the transfer emulator, grounded in a failing behavioral check of this suite.

=== Cause 1 of 1: the InvalidRequestException message for a CUSTOM workflow step with no Lambda Target is emulator-authored prose, not AWS's wording ===

AWS: Smithy CreateWorkflow takes Steps (WorkflowSteps, @required, @length 0..8) and models InvalidRequestException as a client error with httpError 400 ("This exception is thrown when the client submits a malformed request"). The model makes WorkflowStep.CustomStepDetails.Target optional on the wire (CustomStepTarget, @length 0..170, @pattern ^arn:[a-z-]+:lambda:.*$), so the "a CUSTOM step must name a Lambda function" rule is service-side, and the recorded behavior is the requirement: CreateWorkflow with Steps=[{Type:'CUSTOM', CustomStepDetails:{Name:'step-custom'}}] is refused with HTTP 400 InvalidRequestException whose Message is exactly the fixed sentence 'Invalid function provided'. That message has NO variable parts — AWS does not interpolate the step index, the member path or the step name into it; it is fixed text in full. The companion leg of the same check (an unknown step Type='ENCRYPT', refused with ValidationException and the message "1 validation error detected: Value at 'steps.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [DELETE, DECRYPT, CUSTOM, COPY, TAG]") already matches, so only the targetless-CUSTOM message diverges.

Emulator: the call IS refused, and with the right code and status (InvalidRequestException / HTTP 400), but the Message is 'Steps[0].CustomStepDetails.Target must be specified when Type is CUSTOM' — a self-authored diagnostic that interpolates the step index and the member path, neither of which appears in AWS's message for this error class. Replay output: AssertionError: unexpected targetless-CUSTOM message: 'Steps[0].CustomStepDetails.Target must be specified when Type is CUSTOM'.

Why: the error message is part of the contract for this error class. Transfer funnels many semantic workflow-definition defects into the single InvalidRequestException code, so clients, CLI users and test suites discriminate between them on the message text. A client that matches on 'Invalid function provided' to detect specifically "this CUSTOM step has no usable Lambda target" (to fill in a default function ARN, to point the user at the Lambda it must create, or to classify the failure in a provisioning report) never matches against the emulator and falls through to its generic/unknown-error branch, even though the emulator rejected the request for exactly that reason.

Fix: the CreateWorkflow handler's step validation in the transfer service crate (and the identical check on any other path that validates workflow step definitions, e.g. OnExceptionSteps and the same rule under UpdateWorkflow/ workflow-definition validation if present) must raise the modeled InvalidRequestException carrying the fixed message 'Invalid function provided' rather than composing a member-path sentence.

Traces: transfer.create-workflow-rejects-unknown-step-type-and-targetless-custom.

Note: open issue 20261022T202037Z-transfer-input-validation-authorization-and-reference-data-diverge.md covers workflow representation (DescribeWorkflow omitting Tags) and other validation gaps, but does NOT cover this CreateWorkflow rejection message; open issues 20261024T042010Z-transfer-fidelity-gaps-api-gateway-createserver-always-refused.md and 20261026T022407Z-transfer-host-key-and-web-app-planes-still.md own other checks in this suite and do not cover it either.

## Evidence
Cause 1 — CreateWorkflow(Description=<desc>, Steps=[{'Type': 'CUSTOM', 'CustomStepDetails': {'Name': 'step-custom'}}]).

expected: HTTP 400, Error.Code='InvalidRequestException', Error.Message='Invalid function provided' (fixed text in its entirety; no request- or state-derived parts), and no workflow persisted.

actual: HTTP 400, Error.Code='InvalidRequestException' (correct), Error.Message='Steps[0].CustomStepDetails.Target must be specified when Type is CUSTOM'. Fixed text invented by the emulator: 'must be specified when Type is CUSTOM'; parts interpolated from the request that AWS does not interpolate at all: the step index '0' and the member path 'Steps[0].CustomStepDetails.Target'.

Replay assertion: check_transfer_create_workflow_rejects_unknown_step_type_and_targetless_custom.py line 78 — assert msg_target == 'Invalid function provided' -> AssertionError: unexpected targetless-CUSTOM message: 'Steps[0].CustomStepDetails.Target must be specified when Type is CUSTOM'.

The preceding leg of the same check passed: CreateWorkflow with Type='ENCRYPT' returned ValidationException with the exact AWS enum-constraint sentence, so the code/status plumbing is right and only this message's wording is wrong. Every later assertion in the check (no leaked workflow ids, valid COPY baseline created/listed/described as Type='COPY', DeleteWorkflow then DescribeWorkflow -> ResourceNotFoundException 'Unknown workflow') was not reached because the assertion aborted the check.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. tf = boto3.client('transfer', endpoint_url=<emulator>).
3. Call tf.create_workflow(Description='no target', Steps=[{'Type': 'CUSTOM', 'CustomStepDetails': {'Name': 'step-custom'}}]).
4. Catch the ClientError and inspect exc.response['Error']: Code is 'InvalidRequestException' (correct) but Message is 'Steps[0].CustomStepDetails.Target must be specified when Type is CUSTOM' instead of AWS's 'Invalid function provided'.
5. Control, for contrast: tf.create_workflow(Description='bad type', Steps=[{'Type': 'ENCRYPT', 'CopyStepDetails': {...}}]) returns ValidationException with the exact AWS message, showing the divergence is confined to the targetless-CUSTOM rejection message.
