Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.wait-seconds-negative-literal-rejected stepfunctions.sfn-create-state-machine-negative-interval-seconds-rejected stepfunctions.retry-backoffrate-below-one-rejected-at-create-state-machine stepfunctions.map-maxconcurrency-negative-literal-rejected-at-create stepfunctions.create-express-state-machine-ecs-run-task-sync-rejected stepfunctions.create-express-state-machine-sqs-wait-for-task-token-rejected stepfunctions.create-state-machine-invalid-definition-did-you-mean-hint stepfunctions.jsonata-variable-scope-nested-map-parallel-boundaries stepfunctions.alias-routing-config-invalid-routing-configuration-reason-rejected stepfunctions.update-state-machine-encryption-configuration-without-definition-missing-required-parameter stepfunctions.create-state-machine-version-description-without-publish-rejected stepfunctions.update-state-machine-version-description-without-publish-rejected stepfunctions.create-state-machine-invalid-role-arn-format-rejected stepfunctions.update-state-machine-invalid-role-arn-format-rejected stepfunctions.test-state-rejects-invalid-arns-and-unknown-integration stepfunctions.create-state-machine-invalid-type-value-rejected stepfunctions.validate-state-machine-definition-invalid-type-value-rejected stepfunctions.alias-routing-config-length-and-weight-bound-rejected stepfunctions.redrive-execution-client-token-constraint-rejected stepfunctions.sfn-static-variable-validation-field-and-syntax-rejections stepfunctions.sfn-undefined-variable-is-error-not-warning-in-jsonpath-fields stepfunctions.jsonata-malformed-string-and-arguments-schema-validation-messages stepfunctions.jsonata-fail-error-and-cause-each-get-their-own-diagnostic
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.wait-seconds-negative-literal-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected the call to be refused, it succeeded with {result!r}")
Issue-Key: stepfunctions.sfn-create-state-machine-negative-interval-seconds-rejected|_error#4|AssertionError|raise AssertionError(f"expected CreateStateMachine to be refused, but it succeeded: {result!r}")
Issue-Key: stepfunctions.retry-backoffrate-below-one-rejected-at-create-state-machine|_refusal#9|AssertionError|raise AssertionError(f"expected the call to be refused, it succeeded with {result!r}")
Issue-Key: stepfunctions.map-maxconcurrency-negative-literal-rejected-at-create|_create_state_machine_error#4|AssertionError|raise AssertionError(
Issue-Key: stepfunctions.create-express-state-machine-ecs-run-task-sync-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to fail, but it succeeded and returned {result!r}")
Issue-Key: stepfunctions.create-express-state-machine-sqs-wait-for-task-token-rejected|_error#12|AssertionError|raise AssertionError(f"expected the call to fail, but it succeeded and returned {result!r}")
Issue-Key: stepfunctions.create-state-machine-invalid-definition-did-you-mean-hint|_error#4|AssertionError|raise AssertionError(f"expected the call to be refused, it succeeded with {result!r}")
Issue-Key: stepfunctions.jsonata-variable-scope-nested-map-parallel-boundaries|_create_error#3|AssertionError|raise AssertionError(f"CreateStateMachine accepted a definition it rejects: {arn!r}")
Issue-Key: stepfunctions.alias-routing-config-invalid-routing-configuration-reason-rejected|_error#12|AssertionError|raise AssertionError(f"expected this call to be rejected, but it SUCCEEDED: {result!r}")
Issue-Key: stepfunctions.update-state-machine-encryption-configuration-without-definition-missing-required-parameter|_error#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: stepfunctions.create-state-machine-version-description-without-publish-rejected|run#6|AssertionError|assert message == "Version description can only be set when publish is true", (
Issue-Key: stepfunctions.update-state-machine-version-description-without-publish-rejected|run#9|AssertionError|assert message == "Version description can only be set when publish is true", (
Issue-Key: stepfunctions.create-state-machine-invalid-role-arn-format-rejected|run#7|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: stepfunctions.update-state-machine-invalid-role-arn-format-rejected|run#10|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: stepfunctions.test-state-rejects-invalid-arns-and-unknown-integration|run#7|AssertionError|assert message == "Invalid Role Arn: 'invalid'", f"unexpected InvalidArn message: {message!r}"
Issue-Key: stepfunctions.create-state-machine-invalid-type-value-rejected|run#8|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: stepfunctions.validate-state-machine-definition-invalid-type-value-rejected|run#6|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: stepfunctions.alias-routing-config-length-and-weight-bound-rejected|run#9|AssertionError|assert err["Message"] == expected, f"CreateStateMachineAlias {label}: expected {expected!r}, got {err['Message']!r}"
Issue-Key: stepfunctions.redrive-execution-client-token-constraint-rejected|run#6|AssertionError|assert message == expected, (
Issue-Key: stepfunctions.sfn-static-variable-validation-field-and-syntax-rejections|_only_diagnostic#2|AssertionError|assert resp["result"] == "FAIL", f"expected FAIL, got {resp!r}"
Issue-Key: stepfunctions.sfn-undefined-variable-is-error-not-warning-in-jsonpath-fields|run#4|AssertionError|assert bad["result"] == "FAIL", f"expected FAIL for undefined variables, got {bad['result']!r}"
Issue-Key: stepfunctions.jsonata-malformed-string-and-arguments-schema-validation-messages|_diag_fail#5|AssertionError|assert v["result"] == "FAIL", f"ValidateStateMachineDefinition result {v['result']!r}: {v!r}"
Issue-Key: stepfunctions.jsonata-fail-error-and-cause-each-get-their-own-diagnostic|run#5|AssertionError|assert result == "FAIL", f"expected FAIL, got {result!r}: {diags!r}"
Issue-Commit: 4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666

# stepfunctions: 10 net-new causes — CreateStateMachine/UpdateStateMachine/alias/validate accept invalid input and word their rejections wrongly

> _Note: this issue was identified against the code base at commit `4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
Ten independent root causes in the control-plane validation of definitions and request members. Six are missing rejections (the emulator returns 200 for input AWS refuses); four are rejections whose code or message text diverges. Each must be closed separately.

Cause 1 of 10: out-of-range numeric fields in a definition are accepted at create.
AWS: CreateStateMachine rejects InvalidDefinition (Smithy: CreateStateMachine errors include InvalidDefinition, httpError 400) when a definition carries `Wait.Seconds` negative, `Retry.IntervalSeconds` negative, `Retry.BackoffRate` < 1.0, or `Map.MaxConcurrency` negative; no state machine is created.
Emulator: all four succeed with HTTP 200 and a stateMachineArn.
Why: a client's invalid definition is accepted and only misbehaves at runtime, so the validation contract CreateStateMachine exists to provide is gone.
Fix: definition static validation (numeric range checks for Wait/Retry/Map fields) in the CreateStateMachine handler.
Traces: stepfunctions.wait-seconds-negative-literal-rejected, stepfunctions.sfn-create-state-machine-negative-interval-seconds-rejected, stepfunctions.retry-backoffrate-below-one-rejected-at-create-state-machine, stepfunctions.map-maxconcurrency-negative-literal-rejected-at-create

Cause 2 of 10: EXPRESS machines accept integration patterns they cannot run.
AWS: creating an EXPRESS state machine whose definition uses a `.sync` integration (e.g. `arn:aws:states:::ecs:runTask.sync`) or `.waitForTaskToken` is refused at CreateStateMachine; only STANDARD supports them.
Emulator: both CreateStateMachine calls succeed with 200.
Why: the machine is accepted and then cannot work, instead of failing fast where AWS fails.
Fix: the type-vs-integration-pattern check in CreateStateMachine's definition validation.
Traces: stepfunctions.create-express-state-machine-ecs-run-task-sync-rejected, stepfunctions.create-express-state-machine-sqs-wait-for-task-token-rejected

Cause 3 of 10: a near-miss (misspelled) definition field is accepted with no "did you mean" hint.
AWS: a definition carrying a field whose name is a near-miss of a known field is refused InvalidDefinition with a message naming the unknown field and suggesting the intended one.
Emulator: CreateStateMachine succeeds with 200.
Fix: unknown-member detection (with suggestion) in definition validation.
Traces: stepfunctions.create-state-machine-invalid-definition-did-you-mean-hint

Cause 4 of 10: a duplicate variable name across nested Map/Parallel scopes is accepted at create though it is rejected at runtime.
AWS: the definition is refused at CreateStateMachine.
Emulator: CreateStateMachine returns an ARN for a definition the emulator itself later rejects.
Fix: variable-scope validation in definition validation.
Traces: stepfunctions.jsonata-variable-scope-nested-map-parallel-boundaries

Cause 5 of 10: CreateStateMachineAlias accepts an invalid routingConfiguration.
AWS: an invalid routingConfiguration (weights not summing to 100 / an invalid entry) is refused ValidationException with the reason named; no alias is created. Smithy: CreateStateMachineAlias.routingConfiguration is required, RoutingConfigurationList @length min 1 max 2.
Emulator: returns 200 with a stateMachineAliasArn.
Fix: routingConfiguration validation in the CreateStateMachineAlias handler.
Traces: stepfunctions.alias-routing-config-invalid-routing-configuration-reason-rejected

Cause 6 of 10: UpdateStateMachine with only encryptionConfiguration is accepted.
AWS: UpdateStateMachine must carry at least one of definition/roleArn; a request that changes only encryptionConfiguration is refused MissingRequiredParameter. Smithy: UpdateStateMachine errors include MissingRequiredParameter, documented as "This error occurs if both definition and roleArn are not specified."
Emulator: returns 200 with an updateDate and revisionId.
Fix: the required-member precondition in the UpdateStateMachine handler.
Traces: stepfunctions.update-state-machine-encryption-configuration-without-definition-missing-required-parameter

Cause 7 of 10: the versionDescription-without-publish message is wrong.
AWS: ValidationException with message exactly `Version description can only be set when publish is true` on both CreateStateMachine and UpdateStateMachine (fixed text, no variable parts). Smithy: both operations carry `publish` (default false) and `versionDescription`.
Emulator: ValidationException with `versionDescription can only be set when publish is true` (member name instead of AWS's words).
Fix: the message in the publish/versionDescription precondition of both handlers.
Traces: stepfunctions.create-state-machine-version-description-without-publish-rejected, stepfunctions.update-state-machine-version-description-without-publish-rejected

Cause 8 of 10: a malformed roleArn is reported as a generic ARN error.
AWS: a roleArn that is not an ARN is refused InvalidArn with message `Invalid Role Arn: '<the value as submitted>'` — fixed prefix `Invalid Role Arn: ` plus the submitted value — on CreateStateMachine, UpdateStateMachine and TestState (TestState's unknown-integration ARN gets the same documented InvalidArn wording). Smithy: InvalidArn is a modelled error of all three, httpError 400.
Emulator: `Invalid Arn: 'Invalid ARN prefix: invalid-arn'` — wrong prefix and an invented inner reason.
Fix: the roleArn/resource-ARN parse error message in the shared ARN validator used by these handlers.
Traces: stepfunctions.create-state-machine-invalid-role-arn-format-rejected, stepfunctions.update-state-machine-invalid-role-arn-format-rejected, stepfunctions.test-state-rejects-invalid-arns-and-unknown-integration

Cause 9 of 10: the generated validation-error messages are built wrongly (enum order, missing value, error order, pattern rendering).
AWS: for `type` the message is `1 validation error detected: Value 'BOGUS' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [EXPRESS, STANDARD]` — the value set is rendered in that order (note the model declares [STANDARD, EXPRESS], so the order is AWS's own, not the model's); a list member renders its value, `Value '[]' at 'routingConfiguration' failed to satisfy constraint: Member must have length greater than or equal to 1`; and when a member violates several constraints the pattern violation is listed FIRST and the pattern is rendered unanchored, `Value '' at 'clientToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [!-~]+; Value '' at 'clientToken' failed to satisfy constraint: Member must have length greater than or equal to 1`. Smithy: ClientToken @length 1..64 @pattern `^[!-~]+$`; RoutingConfigurationList @length min 1.
Emulator: emits `[STANDARD, EXPRESS]`, omits the value (`Value at 'routingConfiguration' ...`), orders length before pattern, and prints the anchored pattern `^[!-~]+$`.
Why: clients and SDK tests that match these generated messages — the same renderer serves every operation — see a different string on every shape.
Fix: the validation-message renderer (enum ordering, value rendering for list/empty members, multi-violation ordering, pattern un-anchoring).
Traces: stepfunctions.create-state-machine-invalid-type-value-rejected, stepfunctions.validate-state-machine-definition-invalid-type-value-rejected, stepfunctions.alias-routing-config-length-and-weight-bound-rejected, stepfunctions.redrive-execution-client-token-constraint-rejected

Cause 10 of 10: ValidateStateMachineDefinition answers OK for definitions AWS diagnoses.
AWS: ValidateStateMachineDefinition returns result FAIL with one diagnostic per problem for: static variable validation (bad field/syntax), an undefined variable reference in a JSONPath field (an ERROR, not a warning), a malformed JSONata string or Arguments schema violation, and a Fail state's Error and Cause expressions (each gets its own diagnostic). Smithy: output members `result` (enum) and `diagnostics` are both required.
Emulator: returns `{'result': 'OK', 'diagnostics': [], 'truncated': False}` for all of them.
Why: the operation whose only purpose is pre-flight diagnosis reports every bad definition as valid.
Fix: the definition diagnostics engine behind ValidateStateMachineDefinition.
Traces: stepfunctions.sfn-static-variable-validation-field-and-syntax-rejections, stepfunctions.sfn-undefined-variable-is-error-not-warning-in-jsonpath-fields, stepfunctions.jsonata-malformed-string-and-arguments-schema-validation-messages, stepfunctions.jsonata-fail-error-and-cause-each-get-their-own-diagnostic

## Evidence
1) `expected the call to be refused, it succeeded with {'stateMachineArn': '...', 'creationDate': ..., 'HTTPStatusCode': 200}` for Wait Seconds -1, Retry IntervalSeconds negative, BackoffRate < 1, Map MaxConcurrency -1.
2) `expected the call to fail, but it succeeded and returned {'stateMachineArn': '...sim2real-behav-sfn-express-...'}` for an EXPRESS machine with ecs:runTask.sync and with sqs waitForTaskToken.
3) `expected the call to be refused, it succeeded with {'stateMachineArn': '...sim2real-behav-sm-near-...'}`.
4) `CreateStateMachine accepted a definition it rejects: 'arn:aws:states:...:stateMachine:...'`.
5) `expected this call to be rejected, but it SUCCEEDED: {'stateMachineAliasArn': '...', 'HTTPStatusCode': 200}`.
6) `expected the call to be refused, but it succeeded: {'updateDate': ..., 'revisionId': ...}`.
7) expected `Version description can only be set when publish is true`; got `versionDescription can only be set when publish is true`.
8) expected `Invalid Role Arn: 'invalid-arn'`; got `Invalid Arn: 'Invalid ARN prefix: invalid-arn'`.
9) expected `... enum value set: [EXPRESS, STANDARD]`; got `[STANDARD, EXPRESS]`. Expected `Value '[]' at 'routingConfiguration' ...`; got `Value at 'routingConfiguration' ...`. Expected pattern-then-length with `[!-~]+`; got length-then-pattern with `^[!-~]+$`.
10) `ValidateStateMachineDefinition result 'OK': {'result': 'OK', 'diagnostics': [], 'truncated': False}` and `expected FAIL, got 'OK': []`.

## Steps to Reproduce
1. CreateStateMachine with `{"StartAt":"W","States":{"W":{"Type":"Wait","Seconds":-1,"End":true}}}` (and the Retry/BackoffRate/MaxConcurrency variants).
2. CreateStateMachine type=EXPRESS with an `ecs:runTask.sync` Task; repeat with `sqs:sendMessage.waitForTaskToken`.
3. CreateStateMachine with a misspelled state field.
4. CreateStateMachine with the same variable name assigned in nested Map/Parallel scopes.
5. CreateStateMachineAlias with routing weights that do not sum to 100.
6. UpdateStateMachine passing only stateMachineArn + encryptionConfiguration.
7. CreateStateMachine/UpdateStateMachine with versionDescription and publish=false.
8. CreateStateMachine with roleArn='invalid-arn'; same for UpdateStateMachine and TestState.
9. CreateStateMachine with type='BOGUS'; CreateStateMachineAlias with routingConfiguration=[]; RedriveExecution with clientToken=''.
10. ValidateStateMachineDefinition on each of the four bad definitions above.
