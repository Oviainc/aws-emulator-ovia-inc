Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged stepfunctions.create-state-machine-definition-exceeds-size-limit-rejected stepfunctions.test-state-invalid-execution-input-rejected stepfunctions.reject-test-state-invalid-execution-input stepfunctions.delete-state-machine-alias-after-sm-delete-not-found
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: sfn|UpdateStateMachine|InvalidLoggingConfiguration
Issue-Key: stepfunctions.create-state-machine-definition-exceeds-size-limit-rejected|run#22|AssertionError|assert message_malformed == (
Issue-Key: stepfunctions.test-state-invalid-execution-input-rejected|run#9|AssertionError|assert message == _MALFORMED_INPUT_MESSAGE, f"unexpected InvalidExecutionInput message: {message!r}"
Issue-Key: stepfunctions.reject-test-state-invalid-execution-input|run#9|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.delete-state-machine-alias-after-sm-delete-not-found|run#14|AssertionError|assert mutate_message == f"Resource not found: '{alias_arn}'", (
Issue-Commit: 0deae0340a86101143c629cc9b774419f34d5675

# sfn refusal-message fidelity: four net-new causes — InvalidLoggingConfiguration qualifier text, InvalidDefinition JSON locator, TestState InvalidExecutionInput locator (and leaked sensitive input), and the alias-mutation ResourceNotFound after a parent-state-machine delete

> _Note: this issue was identified against the code base at commit `0deae0340a86101143c629cc9b774419f34d5675`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStateMachine, CreateStateMachine, TestState, UpdateStateMachineAlias

## Description
FOUR net-new root causes in the Step Functions refusal vocabulary. All four must be closed before submitting; a fixer should re-replay every check id in trace_ids. In all four the HTTP status and the error CODE are correct — the divergence is the error MESSAGE (and in Cause 3 also a @sensitive member leaked into it). Each of the five failing checks below was left UNMARKED by the replay report (no ALREADY FILED, no OWNED), so no open issue carries their replay keys. Smithy model cited throughout: services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/com-amazonaws-sfn-awsstepfunctions.json.

=== Cause 1 of 4: UpdateStateMachine's InvalidLoggingConfiguration refusal for a log-group ARN without the ':*' qualifier is emulator-authored prose that echoes the submitted ARN ===
Failing check ids: sfn.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged.
AWS: Smithy models UpdateStateMachine with loggingConfiguration -> LoggingConfiguration and binds InvalidLoggingConfiguration (@httpError 400, defaulted) to both UpdateStateMachine and CreateStateMachine. The model does not state the message text, so this is recorded behavior: a loggingConfiguration destination whose cloudWatchLogsLogGroup.logGroupArn is a log-group ARN that does not end in ':*' is refused with HTTP 400, code InvalidLoggingConfiguration and the message "Invalid Logging Configuration: Log Group ARN must be provided with '*' qualifier." That message is ENTIRELY fixed text — it has no variable part and in particular never names the offending ARN, the log group or the state machine.
Emulator: same status and code, but the message is "Invalid Logging Configuration: '<logGroupArn>' is not the ARN of a CloudWatch Logs log group ending with ':*'." — different fixed text, plus the request's ARN interpolated where AWS interpolates nothing. (The ARN seen in the replay, arn:aws:logs:us-west-2:000000000000:log-group:/aws/vendedlogs/states/sim2real-behav-98910ec1820e, is a per-run generated value and is not part of any requirement; the defect is that the template interpolates a request value at all and does not use AWS's sentence.)
Why: deployment tooling and CI guards that pin AWS's documented sentence (the ':*'-qualifier rule is the single most common Step Functions logging misconfiguration) never match, so the refusal is reported as an unknown error instead of "you forgot the '*' qualifier"; operators are also shown a destination ARN where AWS deliberately returns a request-independent sentence.
Fix: the loggingConfiguration validation / InvalidLoggingConfiguration construction shared by the UpdateStateMachine and CreateStateMachine handlers of the sfn service crate.
Traces: sfn.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged (assertion at check_sfn_log_sfn_apigw_agreement_update_to_unqualified_log_group_arn_rejected_and_delivery_unchanged.py line 45).

=== Cause 2 of 4: CreateStateMachine's InvalidDefinition for syntactically malformed JSON drops the parser source locator and inserts an emulator-authored clause ===
Failing check ids: sfn.create-state-machine-definition-exceeds-size-limit-rejected.
AWS: Smithy models CreateStateMachine input member definition -> Definition (@length 1..1048576, @sensitive) and binds InvalidDefinition (@httpError 400, defaulted). The message text is recorded behavior: for a definition that is not parseable JSON, AWS answers HTTP 400 / InvalidDefinition with "Invalid State Machine Definition: '" + a single INVALID_JSON_DESCRIPTION reason + "'", where the reason is the Jackson parser's own text INCLUDING its source locator, i.e. for a definition missing its closing brace: INVALID_JSON_DESCRIPTION: Unexpected end-of-input: expected close marker for Object (start marker at [Source: (String)"<definition>"; line: 1, column: 1])\n at [Source: (String)"<definition>"; line: 1, column: <end>] at [Source: (String)"<definition>"; line: 1, column: <end>]. Fixed text: the "Invalid State Machine Definition: '...'" wrapper, the "INVALID_JSON_DESCRIPTION: " code, the parser sentence ("Unexpected end-of-input: expected close marker for Object"), the "(start marker at [Source: (String)\"...\"; line: 1, column: 1])" clause and the two repeated " at [Source: ...]" locators. Variable parts taken from the request: the definition string echoed inside each [Source: (String)"..."] and the 1-based column numbers (the start marker's column for the unclosed object, and len(definition)+1 for the end-of-input position).
Emulator: HTTP 400 / InvalidDefinition (both correct) with "Invalid State Machine Definition: 'INVALID_JSON_DESCRIPTION: the definition is not valid JSON: Unexpected end-of-input: expected close marker for Object'" — it inserts the emulator-authored clause "the definition is not valid JSON: " that AWS never emits, and omits the start-marker clause and both source/line/column locators entirely.
Why: the locator is the only thing that tells a caller WHERE its definition is broken; a client (console, CDK/Terraform diagnostics, CI linters) that parses or displays the Jackson locator gets nothing, and anything matching AWS's reason text fails on the extra clause. Note the companion assertion in the SAME check now passes: the oversized-definition ValidationException no longer echoes the @sensitive definition, so Cause 7 of the fixed issue 20261029T183117Z-sfn-control-plane-refusal-vocabulary-ten-refusal-families.md is genuinely closed; Cause 8 of that same fixed issue described this INVALID_JSON_DESCRIPTION family for the checks sfn.error-invalid-definition and sfn.test-state-invalid-definition-rejected (both pass now), but the CreateStateMachine malformed-JSON locator it also called for was not delivered and is what this check grades. This is NOT a reopening of a refused call: AWS refuses here too, only the message diverges.
Fix: the ASL validator's INVALID_JSON_DESCRIPTION diagnostic formatting used by the CreateStateMachine handler of the sfn service crate (shared with UpdateStateMachine and TestState, which bind the same InvalidDefinition shape).
Traces: sfn.create-state-machine-definition-exceeds-size-limit-rejected (assertion at check_sfn_create_state_machine_definition_exceeds_size_limit_rejected.py line 102).

=== Cause 3 of 4: TestState's InvalidExecutionInput message appends a raw source locator that echoes the @sensitive input, instead of AWS's redacted start-marker form (and instead of no locator at all for an unrecognized token) ===
Failing check ids: sfn.test-state-invalid-execution-input-rejected, sfn.reject-test-state-invalid-execution-input.
AWS: Smithy models TestState input member input -> SensitiveData (@length 0..262144, @sensitive) and binds InvalidExecutionInput (@httpError 400, defaulted). Message text is recorded behavior, and the two recordings show both forms of AWS's wrapper "Invalid State Machine Execution Input: '<parser reason>'":
  * input that ends mid-object (e.g. an object missing its closing brace) -> reason "Unexpected end-of-input: expected close marker for Object (start marker at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1])". Fixed text: the whole sentence including the literal REDACTED source clause; the only variable parts are the line/column of the start marker. AWS deliberately REDACTS the source because the member is @sensitive, and it appends NO trailing " at [Source: ...]" locator.
  * input whose first token is not a JSON value (e.g. bare text) -> reason "Unrecognized token '<token>': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')" with NO locator clause at all; the variable part is the offending token taken from the request.
Emulator: HTTP 400 / InvalidExecutionInput (both correct) but it appends its own locator that echoes the sensitive input verbatim: "Invalid State Machine Execution Input: 'Unexpected end-of-input: expected close marker for Object\n at [Source: (String)\"{\"probe\": 1\"; line: 1, column: 11]'" and "Invalid State Machine Execution Input: 'Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')\n at [Source: (String)\"this is definitely not { json\"; line: 1, column: 1]'". So it omits AWS's "(start marker at [Source: REDACTED ...])" clause on the end-of-input form, adds a locator AWS never emits on the unrecognized-token form, and in both cases prints the caller's @sensitive input back. (The probe payloads quoted here are the checks' per-run fixtures, not requirements.)
Why: a @sensitive member is leaked into an error message that is logged and surfaced to users — the exact leak AWS avoids by disabling INCLUDE_SOURCE_IN_LOCATION — and any client pinned to either of AWS's two reason forms mismatches. Fixed issue 20261029T183117Z-sfn-control-plane-refusal-vocabulary-ten-refusal-families.md Cause 9 called for a locator to be added to this family; the locator that was added is the raw-source form, which is the opposite of AWS's redaction, so the check still diverges — the call is still correctly refused, so this is not a reopening of a refusal.
Fix: the input-parsing diagnostics / InvalidExecutionInput message construction in the TestState handler of the sfn service crate: redact the source in the locator and emit the start-marker clause only for the parser errors that carry one.
Traces: sfn.test-state-invalid-execution-input-rejected (assertion at check_sfn_test_state_invalid_execution_input_rejected.py line 52), sfn.reject-test-state-invalid-execution-input (line 35).

=== Cause 4 of 4: UpdateStateMachineAlias on an alias whose PARENT state machine was deleted answers the alias-deleted sentence instead of naming the alias ARN ===
Failing check ids: sfn.delete-state-machine-alias-after-sm-delete-not-found.
AWS: Smithy models UpdateStateMachineAlias with input {stateMachineAliasArn (required, Arn, @length 1..256), description, routingConfiguration (@length 1..2)} and errors ConflictException (409), InvalidArn (400), ResourceNotFound (@httpError 404 modeled, 400 on the wire for this awsJson1_0 service), StateMachineDeleting (400), ValidationException (400). The message text is recorded behavior, and the recordings distinguish TWO not-found situations for the same alias-mutation operation:
  * the ALIAS itself was deleted -> message "Request references a resource that does not exist." (wholly fixed text, no variable part). This is what the open issue 20261030T110426Z-sfn-updatestatemachinealias-on-a-deleted-alias-returns-the-alias.md records, grounded in sfn.alias-create-describe-update-delete-lifecycle / sfn.alias-delete-then-describe-not-found / sfn.sm-alias-create-describe-delete — those three checks pass now.
  * the alias was never deleted but its PARENT STATE MACHINE was (DeleteStateMachine, after which the alias becomes unresolvable) -> both the read and the mutation path answer "Resource not found: '<stateMachineAliasArn>'". Fixed text: "Resource not found: '" and the closing "'"; variable part: the alias ARN exactly as supplied in the request.
Emulator: HTTP/code correct, and the READ path is correct in this scenario — the DescribeStateMachineAlias assertion in this same check passes with "Resource not found: '<alias arn>'". But UpdateStateMachineAlias answers "Request references a resource that does not exist." for it, i.e. the alias-deleted sentence is now applied to EVERY alias-mutation ResourceNotFound regardless of which resource vanished. The fix for the open issue above was over-generalised: the fixed sentence belongs only to the deleted-alias case.
Why: a client cannot tell "this alias is gone" from "the state machine behind this alias is gone" — the second is the case a blue/green or weighted-rollout deploy hits after the machine is torn down, and the ARN AWS names is what identifies the dangling alias to clean up. Tooling pinned to either wording now gets the wrong one in one of the two situations.
Fix: the ResourceNotFound construction in the sfn alias handlers: key the wording on WHICH resource was not found (alias record absent -> AWS's fixed sentence; alias present but parent state machine deleted -> "Resource not found: '<requested alias arn>'"), leaving the DescribeStateMachineAlias read rendering untouched.
Traces: sfn.delete-state-machine-alias-after-sm-delete-not-found (assertion at check_sfn_delete_state_machine_alias_after_sm_delete_not_found.py line 57).

## Evidence
Cause 1 — [mismatch] sfn.log-sfn-apigw-agreement-update-to-unqualified-log-group-arn-rejected-and-delivery-unchanged
File "check_sfn_log_sfn_apigw_agreement_update_to_unqualified_log_group_arn_rejected_and_delivery_unchanged.py", line 45, in run / assert message == expected_message
AssertionError: expected message "Invalid Logging Configuration: Log Group ARN must be provided with '*' qualifier.", got "Invalid Logging Configuration: 'arn:aws:logs:us-west-2:000000000000:log-group:/aws/vendedlogs/states/sim2real-behav-98910ec1820e' is not the ARN of a CloudWatch Logs log group ending with ':*'."
Control in the same run: the error CODE assertion (InvalidLoggingConfiguration) passed, so only the message diverges. The log-group ARN in the actual message is a per-run generated value.

Cause 2 — [mismatch] sfn.create-state-machine-definition-exceeds-size-limit-rejected
File "check_sfn_create_state_machine_definition_exceeds_size_limit_rejected.py", line 102, in run / assert message_malformed == (...)
AssertionError: unexpected InvalidDefinition message "Invalid State Machine Definition: 'INVALID_JSON_DESCRIPTION: the definition is not valid JSON: Unexpected end-of-input: expected close marker for Object'"
Expected, for definition '{"StartAt": "Pass", "States": {"Pass": {"Type": "Pass", "End": true}}' (one closing brace missing, length L):
"Invalid State Machine Definition: 'INVALID_JSON_DESCRIPTION: Unexpected end-of-input: expected close marker for Object (start marker at [Source: (String)\"<definition>\"; line: 1, column: 1])\n at [Source: (String)\"<definition>\"; line: 1, column: L+1] at [Source: (String)\"<definition>\"; line: 1, column: L+1]'"
Controls in the same run that PASS: the oversized-definition case returns code ValidationException with message "1 validation error detected: Value at 'definition' failed to satisfy constraint: Member must have length less than or equal to 1048576" (no sensitive echo), the oversized and malformed codes are distinct, neither rejected machine appears in ListStateMachines, and the normal-size valid definition creates an ACTIVE machine whose describe echoes the definition.

Cause 3 — two [mismatch] verdicts, same divergence
1) sfn.test-state-invalid-execution-input-rejected — File "check_sfn_test_state_invalid_execution_input_rejected.py", line 52, in run / assert message == _MALFORMED_INPUT_MESSAGE
Expected: "Invalid State Machine Execution Input: 'Unexpected end-of-input: expected close marker for Object (start marker at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1])'"
Actual: 'Invalid State Machine Execution Input: \'Unexpected end-of-input: expected close marker for Object\n at [Source: (String)"{"probe": 1"; line: 1, column: 11]\''
Control that passes in the same run: TestState with input {"probe": 1} returns status SUCCEEDED and echoes the input, so TestState itself works and the divergence is attributable to the malformed-input refusal path.
2) sfn.reject-test-state-invalid-execution-input — File "check_sfn_reject_test_state_invalid_execution_input.py", line 35, in run / assert message == expected
Expected: "Invalid State Machine Execution Input: 'Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')'"
Actual: 'Invalid State Machine Execution Input: \'Unrecognized token \'this\': was expecting (JSON String, Number, Array, Object or token \'null\', \'true\' or \'false\')\n at [Source: (String)"this is definitely not { json"; line: 1, column: 1]\''
Both: code InvalidExecutionInput and HTTP 400 are correct; the emulator's locator echoes the @sensitive TestState input verbatim.

Cause 4 — [mismatch] sfn.delete-state-machine-alias-after-sm-delete-not-found
File "check_sfn_delete_state_machine_alias_after_sm_delete_not_found.py", line 57, in run / assert mutate_message == f"Resource not found: '{alias_arn}'"
AssertionError: unexpected ResourceNotFound message on update of dead alias 'Request references a resource that does not exist.'
Controls that PASS earlier in the same run: DescribeStateMachineAlias before the parent delete returns the routing configuration; after DeleteStateMachine, the polled DescribeStateMachineAlias is refused with code ResourceNotFound and message "Resource not found: '<alias arn>'" (exactly right); UpdateStateMachineAlias then returns code ResourceNotFound (right) with the wrong message. Expected template: "Resource not found: '" + the stateMachineAliasArn from the request + "'".

## Steps to Reproduce
Cause 1 (UpdateStateMachine / InvalidLoggingConfiguration):
1. iam:CreateRole trusting states.amazonaws.com, with an inline policy allowing the logs:*LogDelivery / logs:PutResourcePolicy / logs:DescribeLogGroups / logs:CreateLogStream / logs:PutLogEvents actions on "*".
2. logs:CreateLogGroup for two groups under /aws/vendedlogs/states/.
3. sfn:CreateStateMachine(..., loggingConfiguration={level: ALL, includeExecutionData: true, destinations: [{cloudWatchLogsLogGroup: {logGroupArn: "<group-1 arn>:*"}}]}).
4. sfn:UpdateStateMachine(stateMachineArn=<sm>, loggingConfiguration={... logGroupArn: "<group-2 arn>"}) — i.e. the same ARN with the trailing ":*" removed.
   Expected: HTTP 400, code InvalidLoggingConfiguration, message exactly "Invalid Logging Configuration: Log Group ARN must be provided with '*' qualifier."
   Observed: same status/code, message "Invalid Logging Configuration: '<unqualified arn>' is not the ARN of a CloudWatch Logs log group ending with ':*'."

Cause 2 (CreateStateMachine / InvalidDefinition):
1. iam:CreateRole trusting states.amazonaws.com.
2. sfn:CreateStateMachine(name=<n>, roleArn=<role>, definition='{"StartAt": "Pass", "States": {"Pass": {"Type": "Pass", "End": true}}') — one closing brace short, so the JSON is unparseable.
   Expected: HTTP 400, code InvalidDefinition, message "Invalid State Machine Definition: 'INVALID_JSON_DESCRIPTION: Unexpected end-of-input: expected close marker for Object (start marker at [Source: (String)\"<definition>\"; line: 1, column: 1])\n at [Source: (String)\"<definition>\"; line: 1, column: <len+1>] at [Source: (String)\"<definition>\"; line: 1, column: <len+1>]'".
   Observed: "Invalid State Machine Definition: 'INVALID_JSON_DESCRIPTION: the definition is not valid JSON: Unexpected end-of-input: expected close marker for Object'" — extra clause, no locator.
3. Confirm the companion controls still hold: the >1048576-character well-formed definition is refused ValidationException without echoing the definition, and a normal valid definition succeeds.

Cause 3 (TestState / InvalidExecutionInput):
1. sfn:TestState(definition='{"Type": "Pass", "End": true}', input='{"probe": 1}') -> status SUCCEEDED, output echoes the input (control).
2. sfn:TestState(definition=same, input='{"probe": 1') -> expect HTTP 400 / InvalidExecutionInput / "Invalid State Machine Execution Input: 'Unexpected end-of-input: expected close marker for Object (start marker at [Source: REDACTED (`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION` disabled); line: 1, column: 1])'". Observed: a "\n at [Source: (String)\"{\"probe\": 1\"; line: 1, column: 11]" locator echoing the input, and no REDACTED start-marker clause.
3. sfn:TestState(definition=same, input='this is definitely not { json') -> expect "Invalid State Machine Execution Input: 'Unrecognized token 'this': was expecting (JSON String, Number, Array, Object or token 'null', 'true' or 'false')'" with NO locator. Observed: the same reason followed by "\n at [Source: (String)\"this is definitely not { json\"; line: 1, column: 1]".

Cause 4 (UpdateStateMachineAlias / ResourceNotFound after parent delete):
1. iam:CreateRole trusting states.amazonaws.com (plus an inline states:* policy).
2. sfn:CreateStateMachine(type=STANDARD, definition=a single Pass state with End:true) -> stateMachineArn.
3. sfn:PublishStateMachineVersion(stateMachineArn) -> v1.
4. sfn:CreateStateMachineAlias(name=<alias>, routingConfiguration=[{stateMachineVersionArn: v1, weight: 100}]) -> stateMachineAliasArn.
5. sfn:DescribeStateMachineAlias(stateMachineAliasArn) -> 200 with that routing (control).
6. sfn:DeleteStateMachine(stateMachineArn) — do NOT delete the alias.
7. Poll sfn:DescribeStateMachineAlias(stateMachineAliasArn) until refused: HTTP 400 / ResourceNotFound / "Resource not found: '<alias arn>'" (correct today, must stay).
8. sfn:UpdateStateMachineAlias(stateMachineAliasArn, routingConfiguration=[{stateMachineVersionArn: v1, weight: 100}]).
   Expected: HTTP 400, code ResourceNotFound, message "Resource not found: '<alias arn>'".
   Observed: HTTP 400, code ResourceNotFound, message "Request references a resource that does not exist." (the deleted-ALIAS wording, applied to the deleted-parent case).
