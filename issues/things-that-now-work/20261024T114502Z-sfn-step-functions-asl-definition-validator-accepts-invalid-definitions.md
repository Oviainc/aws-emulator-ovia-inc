Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.reject-validate-definition-catches-invalid-asl stepfunctions.deep-sfn-2-sfn-jsonata-pass-states-result-rejected stepfunctions.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected stepfunctions.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected stepfunctions.error-kms-access-denied-describe-execution
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.reject-validate-definition-catches-invalid-asl|run#4|AssertionError|assert bad_resp["result"] == "FAIL", f"expected FAIL, got {bad_resp['result']}"
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-pass-states-result-rejected|run#9|AssertionError|assert v["result"] == "FAIL", f"ValidateStateMachineDefinition result {v['result']!r}: {v!r}"
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected|run#10|AssertionError|assert diags == [("ERROR", "UNSUPPORTED_JSONATA_EXPRESSION", "/States/T/Output/cause"),
Issue-Key: stepfunctions.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected|run#9|AssertionError|assert diags == [("ERROR", "SCHEMA_VALIDATION_FAILED", "/States/P")], f"diagnostics {diags!r}"
Issue-Key: stepfunctions.error-kms-access-denied-describe-execution|_refusal_with_retry#35|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# Step Functions: ASL definition validator accepts invalid definitions, misplaces/miscodes JSONata diagnostics, and DescribeExecution never enforces a CMK key policy (5 causes)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ValidateStateMachineDefinition, DescribeExecution

## Description
Five net-new root causes in the Step Functions emulator, all surviving checks that the replay report did NOT mark ALREADY FILED or OWNED. Causes 1-4 live in the shared ASL static validator behind ValidateStateMachineDefinition (and therefore behind CreateStateMachine/UpdateStateMachine, which refuse a definition with the first error of the same walk). Cause 5 is a data-plane authorization gap on DescribeExecution. A fixer must close all five before submitting.

Related open issues that do NOT cover any of these (named here so they are not mistaken for duplicates): 20261024T043150Z-sfn-step-functions-control-plane-no-iam-passrole-authorization.md (control-plane PassRole and Express-only error codes), 20261024T043150Z-sfn-step-functions-interpreter-does-not-run-the-workflow.md (the interpreter not running Task/JSONata), 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md (IAM identity-policy evaluation). None of them is about the ASL static validator's diagnostics, and none is about the KMS key policy being consulted on a Step Functions data-plane read.

=== Cause 1 of 5: ValidateStateMachineDefinition reports result=OK for structurally invalid ASL (unknown state Type, no End/Next) ===
AWS: Smithy: ValidateStateMachineDefinitionOutput.result is @required, target ValidateStateMachineDefinitionResultCode with values {OK, FAIL}, documented "OK when no syntax errors are found, or FAIL if the workflow definition does not pass verification"; ValidateStateMachineDefinitionDiagnostic documents the ERROR codes SCHEMA_VALIDATION_FAILED ("Schema validation reported errors"), MISSING_END and others, with severity @required enum {ERROR, WARNING}. A definition whose only state declares a Type that the Amazon States Language does not define, and which carries neither "Next" nor "End": true, cannot be created, so verification must report result=FAIL with at least one ERROR-severity diagnostic carrying a non-empty code and message.
Emulator: returns HTTP 200 with result="OK" and an empty diagnostics list for such a definition.
Why: a client using ValidateStateMachineDefinition as a pre-deploy or pre-commit gate (the operation's documented purpose) gets a green light for a definition that real CreateStateMachine refuses with InvalidDefinition. The failure is then discovered at deploy time, and CI that asserts result==OK passes on broken workflows.
Fix: the ASL static validator, services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs. The per-state walk reads Type only as "must be a string" and then gates the transition rule on the fixed lists CONTINUING_STATE_TYPES = [Map, Parallel, Pass, Task, Wait] and TERMINAL_STATE_TYPES = [Fail, Succeed]; a Type in neither list falls through every branch, so no diagnostic is produced. There is no "unknown state type" diagnostic at all. services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/validate_state_machine_definition.rs then derives result from that empty diagnostic list, so the handler is correct and the walk is what must reject an unrecognised Type (and require Next/End for it).
Traces: sfn.reject-validate-definition-catches-invalid-asl

=== Cause 2 of 5: $states.result referenced from a JSONata Pass state is accepted ===
AWS: recorded behavior only (the reserved-variable scoping is not in the Smithy model beyond the diagnostic code list). $states.result is bound only in a state that HAS a result (Task, Map, Parallel). For a JSONata machine whose Pass state carries Output {"x": "{% $states.result %}"}, the recording shows ValidateStateMachineDefinition answering result=FAIL with an ERROR diagnostic located at the offending Output field, and CreateStateMachine refusing with InvalidDefinition so that no state machine is left behind. The rule is scoped by state type, not a blanket ban: the recording's positive controls show the identical expression inside a Task's Output validating OK, and the Pass state rewritten to read $states.input creating and running.
Emulator: returns HTTP 200 with result="OK", diagnostics=[], truncated=false, and creates the state machine.
Why: a reference that can never resolve is accepted at deploy time; the client only learns at run time (as a failed execution, or silently as null), which is exactly the class of error this operation exists to catch.
Fix: the JSONata branch of the validator in src/state/asl.rs. It already implements one reserved-variable scope rule ($states.errorOutput, via references_error_output), but has no equivalent for $states.result; RESERVED_TERMS records only the bare "states" term. The scope check belongs next to the errorOutput rule, keyed on the state's Type.
Traces: sfn.deep-sfn-2-sfn-jsonata-pass-states-result-rejected

=== Cause 3 of 5: $states.errorOutput outside a Catch is diagnosed with the wrong code, and once for the whole member instead of once per field ===
AWS: recorded behavior only for the code value. The recording shows two ERROR diagnostics with code UNSUPPORTED_JSONATA_EXPRESSION, located at /States/T/Output/cause and /States/T/Output/error - one per Output field that reads $states.errorOutput. The Smithy model documents the location format as /States/<StateName>/<FieldName>, so a per-field location is the modeled granularity.
Emulator: one ERROR diagnostic, code SCHEMA_VALIDATION_FAILED, location /States/T/Output.
Why: a client that keys off the diagnostic code to distinguish a JSONata expression problem from a schema-shape problem, or that maps diagnostics to editor positions, mislabels and under-reports. Note the model's own caveat ("might add new diagnostics in the future, adjust diagnostic codes, or change the message wording. [...] Do not rely on the exact order, count, or wording") - result is already FAIL here, so this is the LOW-severity end of the bundle and only the code and the per-field location are at issue, not the message text.
Fix: src/state/asl.rs, the references_error_output branch of the JSONata state walk, which pushes a single error(SCHEMA_VALIDATION_FAILED, at(here, "/<member>")) for the whole member. It needs the UNSUPPORTED_JSONATA_EXPRESSION code and one diagnostic per offending leaf field under that member.
Traces: sfn.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected

=== Cause 4 of 5: the JSONPath-state-inside-a-JSONata-machine diagnostic is located on the QueryLanguage member instead of on the state ===
AWS: recorded behavior only. The QueryLanguage override is one-directional - a JSONPath machine may opt single states INTO JSONata, a JSONata machine may not opt a state back out. The recording shows exactly one ERROR diagnostic, code SCHEMA_VALIDATION_FAILED, located at the overriding STATE: /States/P. Severity, code and result (FAIL) all match the emulator; only the location differs.
Emulator: the same single ERROR SCHEMA_VALIDATION_FAILED diagnostic, located at /States/P/QueryLanguage.
Why: a client that groups diagnostics by state, or that resolves the location to a node in its own definition document, attributes the problem to a member rather than to the state the service blames. LOW severity.
Fix: src/state/asl.rs, the declared == JSONPATH && inherited == JSONATA branch of the per-state language check, which builds its location with at(&here, "/QueryLanguage") where the service reports the state itself.
Traces: sfn.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected

=== Cause 5 of 5: DescribeExecution on a CMK-encrypted state machine never returns KmsAccessDeniedException, so the caller's KMS key policy is not enforced ===
AWS: Smithy: DescribeExecution models KmsAccessDeniedException (client error, HTTP 400) alongside KmsInvalidStateException and KmsThrottlingException. Reading the data of an execution of a state machine with encryptionConfiguration type CUSTOMER_MANAGED_KMS_KEY requires the CALLER - not the state machine's execution role - to hold kms:Decrypt on that key; a caller holding states:DescribeExecution but no grant on the CMK must be refused with error code KmsAccessDeniedException and HTTP 400. The refusal must track the key policy: it must appear when the caller's key-policy statement is removed and disappear when it is restored. (The message AWS relays wraps the inner KMS not-authorized text and a per-call request id, so it is an example, not a requirement - the requirement is the 400/KmsAccessDeniedException pair.)
Emulator: the check does not complete - it is cut off at the 60 s cap with no assertion reached, so no response was captured. Hypothesis: the hang is the check's bounded refusal poll (120 s budget, 5 s interval) around the grant-removed DescribeExecution; that loop only exits early on a KmsAccessDeniedException, and every other beat in the check is either a bounded setup retry or a poll that exits on success, so a DescribeExecution that keeps succeeding after the key-policy statement is withdrawn is the only step whose duration exceeds the cap.
Why: a client that relies on CMK encryption as the access boundary for execution data gets no boundary at all - a principal with Step Functions read permission and no grant on the key reads the execution's input and output. Key-policy changes have no observable effect, so neither a denial nor a re-grant can be tested against the emulator.
Fix: the DescribeExecution handler's authorization step in services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/describe_execution.rs, which must consult the key policy of the state machine's encryptionConfiguration.kmsKeyId for the calling principal before returning execution data, and return the modeled KmsAccessDeniedException (400) when the caller has no kms:Decrypt grant. Related but distinct from 20261022T220446Z-iam-no-iam-authorization-is-evaluated-calls-a-session.md: the denial here is sourced from the KMS key policy, not from the caller's IAM identity policy.
Traces: sfn.error-kms-access-denied-describe-execution

## Evidence
Cause 1 - sfn.reject-validate-definition-catches-invalid-asl [mismatch]
  Definition: {"StartAt": "Bogus", "States": {"Bogus": {"Type": "NotARealStateType", "Comment": "invalid type and no End/Next"}}}
  Expected: result == "FAIL" with >=1 ERROR-severity diagnostic (non-empty code and message)
  Actual:   AssertionError: expected FAIL, got OK

Cause 2 - sfn.deep-sfn-2-sfn-jsonata-pass-states-result-rejected [mismatch]
  Definition: QueryLanguage JSONata; Pass state P with Output {"x": "{% $states.result %}"}
  Expected: result == "FAIL" (ERROR diagnostic at the Output field), CreateStateMachine InvalidDefinition
  Actual:   AssertionError: ValidateStateMachineDefinition result 'OK': {'result': 'OK', 'diagnostics': [], 'truncated': False, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}

Cause 3 - sfn.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected [mismatch]
  Definition: JSONata Task T whose own (success-path) Output reads $states.errorOutput in both an 'error' and a 'cause' field
  Expected: [("ERROR", "UNSUPPORTED_JSONATA_EXPRESSION", "/States/T/Output/cause"), ("ERROR", "UNSUPPORTED_JSONATA_EXPRESSION", "/States/T/Output/error")]
  Actual:   AssertionError: diagnostics [('ERROR', 'SCHEMA_VALIDATION_FAILED', '/States/T/Output')]: one per Output field that reads $states.errorOutput

Cause 4 - sfn.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected [mismatch]
  Definition: {"QueryLanguage": "JSONata", "StartAt": "P", "States": {"P": {"Type": "Pass", "QueryLanguage": "JSONPath", "Next": "Q"}, "Q": {...}}}
  Expected: [("ERROR", "SCHEMA_VALIDATION_FAILED", "/States/P")]
  Actual:   AssertionError: diagnostics [('ERROR', 'SCHEMA_VALIDATION_FAILED', '/States/P/QueryLanguage')]

Cause 5 - sfn.error-kms-access-denied-describe-execution [mismatch]
  Expected: DescribeExecution by a caller holding states:DescribeExecution but no kms:Decrypt grant on the state machine's CMK is refused with HTTP 400 / error code KmsAccessDeniedException, and succeeds again once the key-policy statement is restored
  Actual:   Error: Check timed out after 60.0s - no assertion reached, no response captured

Corroborating source read (all four validator causes are visible in one file):
  services/service-com-amazonaws-sfn-awsstepfunctions/src/state/asl.rs
    - TERMINAL_STATE_TYPES = ["Fail", "Succeed"], CONTINUING_STATE_TYPES = ["Map", "Parallel", "Pass", "Task", "Wait"] - an unrecognised Type matches neither, so the Next/End rule never fires and no unknown-type diagnostic exists (Cause 1)
    - RESERVED_TERMS = ["states"] only; no per-state-type scope rule for $states.result (Cause 2)
    - the references_error_output branch pushes error(SCHEMA_VALIDATION_FAILED, at(here, "/<member>"), ...) once per member (Cause 3)
    - the JSONPath-under-JSONata branch pushes at(&here, "/QueryLanguage") (Cause 4)
  services/service-com-amazonaws-sfn-awsstepfunctions/src/ops/validate_state_machine_definition.rs derives result solely from that walk's diagnostics, so OK/FAIL follows the walk.

## Steps to Reproduce
Replay the five failing checks named in trace_ids against the emulator:

  sfn.reject-validate-definition-catches-invalid-asl
  sfn.deep-sfn-2-sfn-jsonata-pass-states-result-rejected
  sfn.deep-sfn-2-sfn-jsonata-error-output-outside-catch-rejected
  sfn.deep-sfn-2-sfn-jsonata-jsonpath-state-under-jsonata-machine-rejected
  sfn.error-kms-access-denied-describe-execution

Causes 1-4 by hand (no setup beyond a Step Functions client):
1. ValidateStateMachineDefinition with definition {"StartAt":"Bogus","States":{"Bogus":{"Type":"NotARealStateType","Comment":"invalid type and no End/Next"}}} -> emulator answers result=OK, diagnostics=[]; expect result=FAIL with an ERROR diagnostic. Sanity: {"StartAt":"P","States":{"P":{"Type":"Pass","End":true}}} must still be OK.
2. ValidateStateMachineDefinition with {"QueryLanguage":"JSONata","StartAt":"Seed","States":{"Seed":{"Type":"Pass","Output":{"n":"{% $states.input.n + 1 %}"},"Next":"P"},"P":{"Type":"Pass","Output":{"x":"{% $states.result %}"},"End":true}}} -> emulator answers result=OK; expect FAIL, and CreateStateMachine on it to raise InvalidDefinition. Control: the same expression in a Task's Output must stay OK.
3. ValidateStateMachineDefinition on a JSONata machine with a Task T whose Output carries both "error" and "cause" fields reading $states.errorOutput -> emulator returns one diagnostic ('ERROR','SCHEMA_VALIDATION_FAILED','/States/T/Output'); expect two, code UNSUPPORTED_JSONATA_EXPRESSION, at /States/T/Output/error and /States/T/Output/cause. Control: the same expression inside the Task's Catch Output must validate OK.
4. ValidateStateMachineDefinition on {"QueryLanguage":"JSONata","StartAt":"P","States":{"P":{"Type":"Pass","QueryLanguage":"JSONPath","Next":"Q"},"Q":{"Type":"Pass","Output":{"v":"{% $states.input.v %}"},"End":true}}} -> diagnostic location is /States/P/QueryLanguage; expect /States/P. Control: the mirror image (JSONPath machine, one JSONata state) must validate OK.

Cause 5 by hand:
1. CreateKey (SYMMETRIC_DEFAULT) with a key policy granting kms:Decrypt/kms:DescribeKey to a second IAM role R, plus the account root and the harness principal.
2. CreateStateMachine (definition {"StartAt":"Pass","States":{"Pass":{"Type":"Pass","End":true}}}) with encryptionConfiguration {type: CUSTOMER_MANAGED_KMS_KEY, kmsKeyId: <key>, kmsDataKeyReusePeriodSeconds: 300}; StartExecution with input "{}" and wait for it to leave RUNNING.
3. AssumeRole into R (R holds only states:DescribeExecution) and call DescribeExecution on that execution -> must succeed (positive control).
4. PutKeyPolicy removing R's statement, leaving the others; call the identical DescribeExecution -> must be refused with HTTP 400 and error code KmsAccessDeniedException. The emulator keeps answering 200, so the check's refusal poll never exits and the check is cut off at the 60 s cap.
5. PutKeyPolicy restoring R's statement; the identical DescribeExecution must succeed again.
