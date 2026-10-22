Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-sap-operation-inspection-validation-errors
Issue-Step: task#1
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-sap-operation-inspection-validation-errors
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap|GetOperation|4xx->4xx|$.message
Issue-Key: ssm-sap|ListOperationEvents|4xx->4xx|$.message
Issue-Key: ssm-sap|ListSubCheckResults|4xx->4xx|$.message
Issue-Key: ssm-sap|ListSubCheckRuleResults|4xx->4xx|$.message
Issue-Key: ssm-sap|GetConfigurationCheckOperation|4xx->4xx|$.message
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# UUID operationId and subCheckResultId pattern messages echo anchored regex

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOperation

## Description
Same shared fault as the applicationId/componentId pattern-message issue, on the UUID-shaped identifier members: a @pattern failure on operationId or subCheckResultId is rendered into the ValidationException message with the model's '^' and '$' anchors still attached, where AWS renders the constraint with the anchors stripped. Status (400) and error type match in every cause; the message is built from the request (it names the member and its constraint), so the rendered constraint is a requirement and the anchored form diverges. Five causes.

Cause 1 of 5: GetOperation / OperationId.
AWS: Smithy OperationId @pattern "^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$"; GetOperation models ValidationException (client, httpError 400) and OperationId is @required. Recorded AWS response for OperationId='not-a-uuid': 400 with message "1 validation error detected: Value 'not-a-uuid' at 'operationId' failed to satisfy constraint: Member must satisfy regular expression pattern: [{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?" — no anchors.
Emulator: 400 with the same text except the pattern is "^[{]?...[}]?$".
Why: a client comparing the reported constraint against AWS's, or reusing it to pre-validate operation ids, gets a pattern AWS never emits.
Fix: the ssm-sap input-validation message renderer (the @pattern -> ValidationException message formatter) must strip the anchoring '^'/'$' before interpolating the model pattern. One change fixes all five causes.
Traces: ssm-sap-operation-inspection-validation-errors#task#1.

Cause 2 of 5: ListOperationEvents / OperationId — same constraint, same anchored-vs-unanchored divergence. Traces: ssm-sap-operation-inspection-validation-errors#task#2.

Cause 3 of 5: ListSubCheckResults / OperationId — same divergence. Traces: ssm-sap-operation-inspection-validation-errors#task#4.

Cause 4 of 5: ListSubCheckRuleResults / subCheckResultId — same UUID pattern, reported at 'subCheckResultId'. AWS echoes the unanchored pattern; emulator echoes "^...$". Traces: ssm-sap-operation-inspection-validation-errors#task#5.

Cause 5 of 5: GetConfigurationCheckOperation / OperationId — same divergence. Traces: ssm-sap-operation-inspection-validation-errors#task#6.

## Evidence
Replay of simparity-wire-aws-v1 (5 mismatch clusters, expected_status 400 == actual_status 400, diverging key $.message):
- GetOperation (#task#1), ListOperationEvents (#task#2), ListSubCheckResults (#task#4), GetConfigurationCheckOperation (#task#6): expected "1 validation error detected: Value 'not-a-uuid' at 'operationId' failed to satisfy constraint: Member must satisfy regular expression pattern: [{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?"; actual the same string with pattern "^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$".
- ListSubCheckRuleResults (#task#5): same pair reported at 'subCheckResultId'.
All five steps are in fixture ssm-sap-operation-inspection-validation-errors.
Model cited: smithy_query shape OperationId -> pattern "^[{]?[0-9a-fA-F]{8}-([0-9a-fA-F]{4}-){3}[0-9a-fA-F]{12}[}]?$"; operation GetOperation -> input OperationId required, errors ValidationException httpError 400.

## Steps to Reproduce
1. Boot the emulator from mainline and replay simparity-wire-aws-v1.
2. Representative step: fixture ssm-sap-operation-inspection-validation-errors, step task#1 — POST /get-operation with OperationId='not-a-uuid'.
3. Observe HTTP 400 whose $.message echoes the anchored UUID pattern.
4. Re-replay steps task#2, task#4, task#5 and task#6 of the same fixture for the other four causes.
