Issue-Source: trace-sweep-divergence
Issue-Trace: ssm-sap-application-lookup-validation-errors
Issue-Step: task#2
Issue-Severity: LOW
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ssm-sap-application-lookup-validation-errors ssm-sap-application-control-errors-on-missing-app ssm-sap-list-components-and-databases ssm-sap-operation-inspection-validation-errors
Issue-Shape-Id: com.amazonaws.ssmsap#SsmSap
Issue-Key: ssm-sap|GetApplication|4xx->4xx|$.message
Issue-Key: ssm-sap|DeregisterApplication|4xx->4xx|$.message
Issue-Key: ssm-sap|GetComponent|4xx->4xx|$.message
Issue-Key: ssm-sap|ListComponents|4xx->4xx|$.message
Issue-Key: ssm-sap|ListDatabases|4xx->4xx|$.message
Issue-Key: ssm-sap|ListOperations|4xx->4xx|$.message
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# ValidationException pattern message echoes anchored regex for applicationId and componentId

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetApplication

## Description
One shared fault with six causes: when an ssm-sap input member fails its Smithy @pattern constraint, the emulator renders the constraint into the ValidationException message VERBATIM from the model, including the leading '^' and trailing '$' anchors. AWS's validation framework renders the same constraint with the anchors stripped. Status (400) and error type match in every cause; only the rendered constraint differs, and that rendering is a known rule (the message is built from the request: it names the member and its constraint), so the anchored form is wrong, not merely differently worded.

Cause 1 of 6: GetApplication / ApplicationId.
AWS: Smithy ApplicationId @length 1..60, @pattern "^[\\w\\d\\.-]+$"; error ValidationException (client, httpError 400). Recorded AWS response: 400 with message "1 validation error detected: Value 'bad app id!' at 'applicationId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w\\d\\.-]+" — the pattern is echoed without anchors.
Emulator: 400, message "... regular expression pattern: ^[\\w\\d\\.-]+$" — the model's anchors are left in.
Why: a client that parses or asserts on the constraint AWS reports (or that re-uses the reported pattern to pre-validate ids locally) sees a pattern AWS never emits; the anchored form is not interchangeable with the AWS text.
Fix: the ssm-sap input-validation message renderer — the component that formats a @pattern failure into the ValidationException message must strip the anchoring '^' and '$' from the model pattern before interpolating it. One change fixes all six causes.
Traces: ssm-sap-application-lookup-validation-errors#task#2.

Cause 2 of 6: DeregisterApplication / ApplicationId. Same model constraint (@pattern "^[\\w\\d\\.-]+$", ValidationException 400). AWS echoes "[\\w\\d\\.-]+" for value 'bad id!!'; emulator echoes "^[\\w\\d\\.-]+$". Traces: ssm-sap-application-control-errors-on-missing-app#task#5.

Cause 3 of 6: GetComponent / ComponentId. Smithy ComponentId @length 1..100, @pattern "^[\\w\\d-]+$". AWS echoes "[\\w\\d-]+" at 'componentId'; emulator echoes "^[\\w\\d-]+$". Traces: ssm-sap-application-lookup-validation-errors#task#3.

Cause 4 of 6: ListComponents / ApplicationId. AWS echoes "[\\w\\d\\.-]+"; emulator echoes the anchored form. Traces: ssm-sap-list-components-and-databases#task#3.

Cause 5 of 6: ListDatabases / ApplicationId. Same divergence. Traces: ssm-sap-list-components-and-databases#task#4.

Cause 6 of 6: ListOperations / ApplicationId. Same divergence. Traces: ssm-sap-operation-inspection-validation-errors#task#3.

## Evidence
Replay of simparity-wire-aws-v1 (6 mismatch clusters, all verdict=mismatch, expected_status 400 == actual_status 400, diverging key $.message):
- GetApplication (ssm-sap-application-lookup-validation-errors#task#2): expected "1 validation error detected: Value 'bad app id!' at 'applicationId' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\w\\d\\.-]+"; actual "... pattern: ^[\\w\\d\\.-]+$".
- DeregisterApplication (ssm-sap-application-control-errors-on-missing-app#task#5): expected "... Value 'bad id!!' at 'applicationId' ... pattern: [\\w\\d\\.-]+"; actual "... pattern: ^[\\w\\d\\.-]+$".
- GetComponent (ssm-sap-application-lookup-validation-errors#task#3): expected "... Value 'bad id!' at 'componentId' ... pattern: [\\w\\d-]+"; actual "... pattern: ^[\\w\\d-]+$".
- ListComponents (ssm-sap-list-components-and-databases#task#3) and ListDatabases (ssm-sap-list-components-and-databases#task#4): expected "... Value 'bad app id!' at 'applicationId' ... pattern: [\\w\\d\\.-]+"; actual anchored form.
- ListOperations (ssm-sap-operation-inspection-validation-errors#task#3): same expected/actual pair.
Model cited: smithy_query shape ApplicationId -> pattern "^[\\w\\d\\.-]+$"; shape ComponentId -> pattern "^[\\w\\d-]+$"; operations GetApplication/DeregisterApplication/ListComponents/ListDatabases/ListOperations all model ValidationException httpError 400.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus simparity-wire-aws-v1.
2. Representative step: fixture ssm-sap-application-lookup-validation-errors, step task#2 — POST /get-application with ApplicationId='bad app id!'.
3. Observe HTTP 400 whose $.message echoes the regex as ^[\\w\\d\\.-]+$ instead of [\\w\\d\\.-]+.
4. Re-replay the other bundled steps for the remaining causes: ssm-sap-application-control-errors-on-missing-app#task#5, ssm-sap-application-lookup-validation-errors#task#3, ssm-sap-list-components-and-databases#task#3 and #task#4, ssm-sap-operation-inspection-validation-errors#task#3.
