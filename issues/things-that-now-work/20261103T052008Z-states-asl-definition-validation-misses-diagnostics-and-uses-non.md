Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-validate-retry-catch-definition-diagnostics
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-validate-retry-catch-definition-diagnostics sfn-invalid-asl-definition-rejected-at-create
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.diagnostics[*]
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.result
Issue-Key: states|ValidateStateMachineDefinition|2xx->2xx|$.diagnostics[*].location
Issue-Key: states|CreateStateMachine|4xx->2xx|$.__type
Issue-Key: states|CreateStateMachine|4xx->2xx|$.message
Issue-Key: states|CreateStateMachine|4xx->2xx|status
Issue-Key: states|UpdateStateMachine|4xx->4xx|$.message
Issue-Key: states|CreateStateMachine|4xx->4xx|$.message
Issue-Commit: e331167544f8c61f25de403c8943054d68a37733

# ASL definition validation misses diagnostics and uses non-AWS index syntax

> _Note: this issue was identified against the code base at commit `e331167544f8c61f25de403c8943054d68a37733`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateStateMachineDefinition

## Description
The ASL definition validator accepts definitions AWS rejects, and where it does report a problem it addresses array elements with a slash-index path instead of AWS's bracket index. The same validator backs ValidateStateMachineDefinition, CreateStateMachine and UpdateStateMachine, so all three diverge. 5 causes.

Cause 1 of 5 — ValidateStateMachineDefinition reports result OK with no diagnostics for definitions whose schema is invalid.
AWS: Smithy: ValidateStateMachineDefinition.output.result -> ValidateStateMachineDefinitionResultCode (required) and .diagnostics -> list of ValidateStateMachineDefinitionDiagnostic (required), whose members are severity, code, message and location. The modelled doc lists SCHEMA_VALIDATION_FAILED as an error code ("Schema validation reported errors") and documents location as "/States/<StateName>/<FieldName>". A definition using a field that the state type or the selected StateMachineType does not support yields result FAIL and a diagnostic {severity ERROR, code SCHEMA_VALIDATION_FAILED, location /States/<StateName>, message "Field '<FieldName>' is not supported"} — fixed text "Field '" + the offending field name + "' is not supported".
Emulator: result OK and diagnostics empty, for eight distinct invalid definitions in the fixture.
Why: the validator is the pre-flight check a deployment pipeline runs; reporting OK lets a definition AWS will reject reach CreateStateMachine, so the failure surfaces at deploy time instead of validation time.
Fix: the ValidateStateMachineDefinition handler's schema validation — it must run the per-state-type field whitelist and emit a diagnostic per violation.
Traces: sfn-validate-retry-catch-definition-diagnostics#task#3

Cause 2 of 5 — a diagnostic's location addresses an array element as /Catch/0/Next instead of /Catch[0]/Next.
AWS: recorded behavior only — AWS's diagnostic location for an element of the Retry/Catch arrays uses a bracketed index appended to the array's field name: /States/<StateName>/Catch[0]/Next, /States/<StateName>/Retry[0]/ErrorEquals[0].
Emulator: /States/<StateName>/Catch/0/Next — the index is a separate slash-delimited segment.
Why: a client that parses the location to highlight the offending element in the submitted document (or that matches the segment names) resolves the wrong node, because `Catch/0` is not a path AWS ever emits.
Fix: the validator's location/JSON-pointer formatter, shared by all diagnostics.
Traces: sfn-validate-retry-catch-definition-diagnostics#task#9

Cause 3 of 5 — CreateStateMachine accepts a definition whose Retry ErrorEquals puts States.ALL before other error names.
AWS: Smithy: CreateStateMachine models InvalidDefinition (HTTP 400, __type com.amazonaws.swf.service.v2.model#InvalidDefinition). The ASL rule is that States.ALL must be the only entry of an ErrorEquals list and must be last; a violation is refused with "Invalid State Machine Definition: '<CODE>: <message> at <location>'" — here code SCHEMA_VALIDATION_FAILED and message "States.ALL must appear alone and at end of list", located at /States/<StateName>/Retry[0]/ErrorEquals[0].
Emulator: HTTP 200 — the state machine is created, and no __type/message is returned at all.
Why: the emulator creates a state machine AWS would refuse, so a test that asserts the definition is rejected passes against the emulator and fails against AWS; and the resulting resource has retry semantics AWS cannot reproduce.
Fix: CreateStateMachine's definition validation must run the same ErrorEquals ordering rule as the validator.
Traces: sfn-validate-retry-catch-definition-diagnostics#task#15

Cause 4 of 5 — UpdateStateMachine's InvalidDefinition message uses the same wrong index syntax as cause 2.
AWS: message "Invalid State Machine Definition: 'MISSING_TRANSITION_TARGET: Missing 'Next' target: <TargetStateName> at /States/<StateName>/Catch[0]/Next'" — fixed text "Invalid State Machine Definition: '", the diagnostic code, "Missing 'Next' target: "; variable: the unresolved target state name and the location, whose array element is bracket-indexed.
Emulator: identical except the location reads /States/<StateName>/Catch/0/Next.
Why: same as cause 2, in a refusal message a client may parse or display.
Fix: the shared location formatter (cause 2), as reached from UpdateStateMachine's definition validation.
Traces: sfn-validate-retry-catch-definition-diagnostics#task#17

Cause 5 of 5 — CreateStateMachine's InvalidDefinition message reports one diagnostic under the wrong field name and omits the unreachable-state diagnostic.
AWS: when a definition has several MISSING_TRANSITION_TARGET problems, AWS concatenates one "<CODE>: <message> at <location>" clause per diagnostic, comma-separated, inside the single quotes: "Invalid State Machine Definition: 'MISSING_TRANSITION_TARGET: Missing 'Next' target: <StartAtTarget> at /StartAt, MISSING_TRANSITION_TARGET: State \"<StateName>\" is not reachable. at /States/<StateName>'". Note AWS names the field 'Next' even for the /StartAt location, and reports the unreachable state as a second diagnostic.
Emulator: a single clause, with the field named 'StartAt' instead of 'Next', and no unreachable-state clause.
Why: a client fixing a definition from the message fixes one problem, resubmits, and is refused again for the state it was never told about; the quoted field name does not match AWS's.
Fix: CreateStateMachine's definition validation — collect all diagnostics into the message, and use AWS's field naming for the StartAt target.
Traces: sfn-invalid-asl-definition-rejected-at-create#task#2

## Evidence
All five causes are mismatch clusters from the replay of simparity-wire-aws-v1 against the emulator built from this checkout.

Cause 1 (sfn-validate-retry-catch-definition-diagnostics#task#3; the cluster also covers #task#4, #task#5, #task#6, #task#7, #task#8, #task#10 and #task#11 — eight steps, all HTTP 200 expected and returned): $.diagnostics[0]: expected {'code': 'SCHEMA_VALIDATION_FAILED', 'location': '/States/Validate', 'message': "Field 'Retry' is not supported", 'severity': 'ERROR'}, got None; $.result: expected 'FAIL', got 'OK'.

Cause 2 (#task#9, HTTP 200): $.diagnostics[0].location: expected '/States/InvokeProcessingLambda/Catch[0]/Next', got '/States/InvokeProcessingLambda/Catch/0/Next'.

Cause 3 (#task#15): status: expected 400, got 200; $.__type: expected 'com.amazonaws.swf.service.v2.model#InvalidDefinition', got None; $.message: expected "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: States.ALL must appear alone and at end of list at /States/InvokeProcessingLambda/Retry[0]/ErrorEquals[0]'", got None.

Cause 4 (#task#17, HTTP 400 expected and returned): $.message: expected "Invalid State Machine Definition: 'MISSING_TRANSITION_TARGET: Missing 'Next' target: HandleError at /States/InvokeProcessingLambda/Catch[0]/Next'", got "...at /States/InvokeProcessingLambda/Catch/0/Next'".

Cause 5 (sfn-invalid-asl-definition-rejected-at-create#task#2, HTTP 400 expected and returned): $.message: expected "Invalid State Machine Definition: 'MISSING_TRANSITION_TARGET: Missing 'Next' target: Bootstrap at /StartAt, MISSING_TRANSITION_TARGET: State \"Normalize\" is not reachable. at /States/Normalize'", got "Invalid State Machine Definition: 'MISSING_TRANSITION_TARGET: Missing 'StartAt' target: Bootstrap at /StartAt'".

## Steps to Reproduce
1. Replay fixture sfn-validate-retry-catch-definition-diagnostics against the emulator. Step task#3 (the representative) calls ValidateStateMachineDefinition with a definition that uses an unsupported field on a state; AWS answers result FAIL with one SCHEMA_VALIDATION_FAILED diagnostic, the emulator answers result OK with no diagnostics. Steps task#4-task#8, task#10 and task#11 are the same defect with other invalid definitions.
2. Step task#9 of the same fixture produces a diagnostic whose location points into the Catch array; compare the location string's index syntax.
3. Step task#15 calls CreateStateMachine with States.ALL first in a Retry ErrorEquals; the recording is HTTP 400 InvalidDefinition, the emulator answers 200.
4. Step task#17 calls UpdateStateMachine with an unresolvable Catch Next target; compare the location in $.message.
5. Replay sfn-invalid-asl-definition-rejected-at-create and compare step task#2's $.message against the recorded two-diagnostic message.
