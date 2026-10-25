Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.create-workflow-rejects-unknown-step-type-and-targetless-custom
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.create-workflow-rejects-unknown-step-type-and-targetless-custom|run#8|AssertionError|assert msg_type == (
Issue-Commit: ddc7fba03bf570c17ccb819546a23e1466a86c54

# CreateWorkflow enum-constraint ValidationException message lists the WorkflowStepType value set in model-declaration order instead of AWS's order

> _Note: this issue was identified against the code base at commit `ddc7fba03bf570c17ccb819546a23e1466a86c54`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWorkflow

## Description
One root cause, grounded in failing behavioral check transfer.create-workflow-rejects-unknown-step-type-and-targetless-custom.

=== Cause 1 of 1: the enum value set rendered in the constraint-violation message is in the wrong order ===

AWS: Smithy CreateWorkflow.Steps -> WorkflowSteps (@length 0..8) whose member WorkflowStep.Type targets the enum com.amazonaws.transfer#WorkflowStepType, declared in the model with values [COPY, CUSTOM, TAG, DELETE, DECRYPT]. A Type outside that set is a client-side constraint violation, refused with HTTP 400 and error code ValidationException (the framework-level constraint error, not the service's modeled InvalidRequestException). Recorded behavior: AWS words the message as the standard constraint sentence "<N> validation error detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]", and for this member the value list is rendered as [DELETE, DECRYPT, CUSTOM, COPY, TAG] — i.e. NOT the model's declaration order. Fixed text of the message: "1 validation error detected: Value at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]". Variable parts: the error count, the member path derived from the request ('steps.1.member.type' for the first element of Steps), and the enum value list, which for WorkflowStepType is the fixed service-side order DELETE, DECRYPT, CUSTOM, COPY, TAG.

Emulator: CreateWorkflow with Steps=[{Type: 'ENCRYPT', CopyStepDetails: {...}}] is correctly refused with code ValidationException and the correct member path, but the message reads "1 validation error detected: Value at 'steps.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [COPY, CUSTOM, TAG, DELETE, DECRYPT]" — the value set is emitted in Smithy declaration order rather than the order AWS renders.

Why: the enum value set is fixed text of the message, not a per-run value, so a client (SDK test, error-message assertion, log-scraping tool, or a UI that surfaces the service's own diagnostic verbatim) that compares the full AWS sentence sees a mismatch, and anything that parses the bracketed list positionally gets a different ordering than real Transfer returns.

Fix: the shared enum-constraint violation renderer in the validate stage of the request walk (smithy_server_runtime's constraint-message formatting, as reached for WorkflowStepType from services/service-com-amazonaws-transfer-transferservice/src/ops/create_workflow.rs via INPUT_SCHEMA / super::types::CREATE_WORKFLOW_INPUT_SCHEMA) — the order in which it lists the enum's allowed values must be the service's rendered order, not the model's member order.

Note: the check aborted at this assertion, so the two later legs of the same check (a CUSTOM step with no Target expected to yield InvalidRequestException "Invalid function provided", and the ResourceNotFoundException "Unknown workflow" message after DeleteWorkflow) were never exercised and are NOT claimed here. Open issue 20261022T202037Z-transfer-input-validation-authorization-and-reference-data-diverge.md covers other transfer validation/authorization/reference-data gaps (including a DescribeWorkflow Tags representation gap) but does not cover this constraint-message rendering.

## Evidence
Check transfer.create-workflow-rejects-unknown-step-type-and-targetless-custom, assertion at line 74 of check_transfer_create_workflow_rejects_unknown_step_type_and_targetless_custom.py:

Request: transfer.create_workflow(Description=<description>, Steps=[{'Type': 'ENCRYPT', 'CopyStepDetails': {'Name': 'step-copy', 'DestinationFileLocation': {'S3FileLocation': {'Bucket': <bucket>, 'Key': 'processed/'}}}}])

Expected (AWS): error code ValidationException with message
  "1 validation error detected: Value at 'steps.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [DELETE, DECRYPT, CUSTOM, COPY, TAG]"

Actual (emulator): error code ValidationException (correct) with message
  "1 validation error detected: Value at 'steps.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [COPY, CUSTOM, TAG, DELETE, DECRYPT]"

AssertionError: unexpected unknown-step-type message: "1 validation error detected: Value at 'steps.1.member.type' failed to satisfy constraint: Member must satisfy enum value set: [COPY, CUSTOM, TAG, DELETE, DECRYPT]"

The divergence is exactly the ordering of the enum value list; status, error code, the fixed sentence text and the request-derived member path all match. Smithy declaration order for com.amazonaws.transfer#WorkflowStepType is COPY, CUSTOM, TAG, DELETE, DECRYPT, which is what the emulator echoes.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. Call transfer.create_workflow(Description='bad type', Steps=[{'Type': 'ENCRYPT', 'CopyStepDetails': {'Name': 'step-copy', 'DestinationFileLocation': {'S3FileLocation': {'Bucket': '<any-bucket>', 'Key': 'processed/'}}}}]).
3. Catch the ClientError and inspect response['Error']['Code'] (ValidationException, correct) and response['Error']['Message'].
4. Observe the enum value set rendered as [COPY, CUSTOM, TAG, DELETE, DECRYPT] where AWS renders [DELETE, DECRYPT, CUSTOM, COPY, TAG] in the same sentence.
