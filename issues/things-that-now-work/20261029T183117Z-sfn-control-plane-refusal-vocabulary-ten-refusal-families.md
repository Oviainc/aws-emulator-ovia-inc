Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.error-resource-not-found-describe-alias stepfunctions.alias-delete-then-describe-not-found stepfunctions.delete-state-machine-alias-after-sm-delete-not-found stepfunctions.delete-state-machine-alias-conflict stepfunctions.sm-alias-create-describe-delete stepfunctions.concurrent-alias-update-delete-race stepfunctions.describe-map-run-not-found stepfunctions.describe-map-run-invalid-arn stepfunctions.update-map-run-resource-not-found stepfunctions.tag-untagged-resource-not-found stepfunctions.create-alias-conflict-duplicate-name stepfunctions.create-alias-numeric-name-rejected stepfunctions.reject-create-state-machine-alias-invalid-name stepfunctions.create-state-machine-alias-invalid-routing-weights-fails stepfunctions.update-state-machine-alias-invalid-routing-weights-rejected stepfunctions.delete-state-machine-version-referenced-by-alias-rejected stepfunctions.delete-version-referenced-by-alias-conflict stepfunctions.error-conflict-delete-state-machine-version-in-use stepfunctions.delete-state-machine-version-validation-guard stepfunctions.create-state-machine-definition-exceeds-size-limit-rejected stepfunctions.error-invalid-definition stepfunctions.test-state-invalid-definition-rejected stepfunctions.test-state-invalid-execution-input-rejected stepfunctions.update-state-machine-missing-required-parameter stepfunctions.update-state-machine-missing-required-parameter-fails
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.error-resource-not-found-describe-alias|run#14|AssertionError|assert message == expected_message, (
Issue-Key: stepfunctions.alias-delete-then-describe-not-found|run#18|AssertionError|assert err == expected, f"DescribeStateMachineAlias on a deleted alias: expected {expected}, got {err}"
Issue-Key: stepfunctions.delete-state-machine-alias-after-sm-delete-not-found|run#11|AssertionError|assert message == f"Resource not found: '{alias_arn}'", f"unexpected ResourceNotFound message {message!r}"
Issue-Key: stepfunctions.delete-state-machine-alias-conflict|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.sm-alias-create-describe-delete|run#14|AssertionError|assert error.get("Message") == expected_message, (
Issue-Key: stepfunctions.concurrent-alias-update-delete-race|run#15|AssertionError|assert message == f"Resource not found: '{alias_arn}'", (
Issue-Key: stepfunctions.describe-map-run-not-found|run#11|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.describe-map-run-invalid-arn|run#7|AssertionError|assert control_message == expected, f"expected message {expected!r}, got {control_message!r}"
Issue-Key: stepfunctions.update-map-run-resource-not-found|run#7|AssertionError|assert message == f"Resource not found: '{bogus}'", f"unexpected ResourceNotFound message: {message!r}"
Issue-Key: stepfunctions.tag-untagged-resource-not-found|run#17|AssertionError|assert message == f"Resource not found: '{arn}'", f"unexpected ResourceNotFound message: {message!r}"
Issue-Key: stepfunctions.create-alias-conflict-duplicate-name|run#11|AssertionError|assert message == (
Issue-Key: stepfunctions.create-alias-numeric-name-rejected|run#8|AssertionError|assert code == "ValidationException", f"expected ValidationException for a purely numeric alias name, got {code}"
Issue-Key: stepfunctions.reject-create-state-machine-alias-invalid-name|run#14|AssertionError|assert code == "ValidationException", f"expected ValidationException, got {code}"
Issue-Key: stepfunctions.create-state-machine-alias-invalid-routing-weights-fails|run#14|AssertionError|assert message == "Sum of routing configuration weights must equal 100. Received: [42]", (
Issue-Key: stepfunctions.update-state-machine-alias-invalid-routing-weights-rejected|run#14|AssertionError|assert message == "Sum of routing configuration weights must equal 100. Received: [40]", (
Issue-Key: stepfunctions.delete-state-machine-version-referenced-by-alias-rejected|run#15|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.delete-version-referenced-by-alias-conflict|run#16|AssertionError|assert message == _conflict_message(alias_name), (
Issue-Key: stepfunctions.error-conflict-delete-state-machine-version-in-use|run#14|AssertionError|assert message == expected_message, (
Issue-Key: stepfunctions.delete-state-machine-version-validation-guard|run#17|AssertionError|assert message == expected, f"expected message {expected!r}, got {message!r}"
Issue-Key: stepfunctions.create-state-machine-definition-exceeds-size-limit-rejected|run#13|AssertionError|assert message_oversized == (
Issue-Key: stepfunctions.error-invalid-definition|run#13|AssertionError|assert message == expected_message, (
Issue-Key: stepfunctions.test-state-invalid-definition-rejected|run#9|AssertionError|assert error.get("Message") == "The state definition must be valid JSON", (
Issue-Key: stepfunctions.test-state-invalid-execution-input-rejected|run#9|AssertionError|assert message == _MALFORMED_INPUT_MESSAGE, f"unexpected InvalidExecutionInput message: {message!r}"
Issue-Key: stepfunctions.update-state-machine-missing-required-parameter|run#7|AssertionError|assert message == _MISSING_MESSAGE, f"expected message {_MISSING_MESSAGE!r}, got {message!r}"
Issue-Key: stepfunctions.update-state-machine-missing-required-parameter-fails|run#9|AssertionError|assert message == _MISSING_MESSAGE, f"expected message {_MISSING_MESSAGE!r}, got {message!r}"
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# sfn control-plane refusal vocabulary: ten refusal families answer emulator-authored prose (and in two cases the wrong error code) instead of AWS's messages (10 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStateMachineAlias

## Description
Ten net-new root causes, all in the Step Functions control plane's refusal vocabulary. In each the status and error code are stated below; where the code is right, the message is emulator-authored prose rather than AWS's wording, so clients that classify or display refusals by message diverge.

Cause 1 of 10: every ResourceNotFound refusal uses the message `could not find the resource <arn>` instead of AWS's `Resource not found: '<arn>'`.
  AWS: Smithy models ResourceNotFound (httpError 404) on DescribeStateMachineAlias, DeleteStateMachineAlias, UpdateStateMachineAlias, DescribeMapRun, UpdateMapRun, TagResource and UntagResource. AWS's message template is the fixed text `Resource not found: '` + the requested ARN from the request + `'`.
  Emulator: `could not find the resource <arn>` — different fixed text, no quoting.
  Why: clients that match on AWS's not-found text (log scrapers, user-facing error surfaces, test fixtures) see an unrecognised message for every missing alias, map run or taggable resource.
  Fix: the ResourceNotFound message construction shared by the alias, map-run and tagging handlers of the sfn service crate.
  Checks: .error-resource-not-found-describe-alias, .alias-delete-then-describe-not-found, .delete-state-machine-alias-after-sm-delete-not-found, .delete-state-machine-alias-conflict, .sm-alias-create-describe-delete, .concurrent-alias-update-delete-race, .describe-map-run-not-found, .describe-map-run-invalid-arn, .update-map-run-resource-not-found, .tag-untagged-resource-not-found.

Cause 2 of 10: CreateStateMachineAlias's duplicate-name ConflictException message.
  AWS: Smithy CreateStateMachineAlias errors include ConflictException (409). AWS's message is the fixed text `Failed to create alias because an alias with the same name and a different routing configuration already exists.` — no variable part.
  Emulator: `State Machine Alias Already Exists: '<alias arn>'` (code ConflictException is correct).
  Fix: the conflict path of the CreateStateMachineAlias handler in the sfn service crate.
  Check: .create-alias-conflict-duplicate-name.

Cause 3 of 10: an alias name that violates the modelled name pattern is refused with the wrong error code and message.
  AWS: Smithy CreateStateMachineAlias input member `name` targets CharacterRestrictedName with @length 1-80 and @pattern `^(?=.*[a-zA-Z_\-\.])[a-zA-Z0-9_\-\.]+$`; a value failing it is refused with ValidationException (400) and the constraint-violation template `1 validation error detected: Value '<name>' at 'name' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(?=.*[a-zA-Z_\-\.])[a-zA-Z0-9_\-\.]+$` — fixed text plus the offending name from the request.
  Emulator: code `InvalidName` (a different modelled error) instead of ValidationException.
  Fix: name validation in the CreateStateMachineAlias handler of the sfn service crate.
  Checks: .create-alias-numeric-name-rejected, .reject-create-state-machine-alias-invalid-name.

Cause 4 of 10: the routing-weight-sum ValidationException message.
  AWS: `Sum of routing configuration weights must equal 100. Received: [<weights>]` — fixed text plus the submitted weights from the request.
  Emulator: `the weights of a routing configuration must add up to 100, got <n>`.
  Fix: routingConfiguration validation shared by CreateStateMachineAlias and UpdateStateMachineAlias in the sfn service crate.
  Checks: .create-state-machine-alias-invalid-routing-weights-fails, .update-state-machine-alias-invalid-routing-weights-rejected.

Cause 5 of 10: the version-referenced-by-an-alias ConflictException message on DeleteStateMachineVersion.
  AWS: Smithy DeleteStateMachineVersion errors include ConflictException (409). Message: `Version to be deleted must not be referenced by an alias. Current list of aliases referencing this version: [<alias names>]` — fixed text plus the referencing alias NAMES from state.
  Emulator: `'<version arn>' is referenced by the state machine alias(es) <alias arns>, which must be deleted or pointed at another version before it can be deleted` — different text, and it names full alias ARNs where AWS names bare alias names.
  Fix: the conflict path of the DeleteStateMachineVersion handler in the sfn service crate.
  Checks: .delete-state-machine-version-referenced-by-alias-rejected, .delete-version-referenced-by-alias-conflict, .error-conflict-delete-state-machine-version-in-use.

Cause 6 of 10: DeleteStateMachineVersion on an unqualified (version-less) state-machine ARN.
  AWS: message `Arn missing version qualifier: <arn>` — fixed text plus the submitted ARN.
  Emulator: `Invalid Arn: 'Invalid ARN stateMachine version: <arn>'`.
  Fix: ARN-qualifier validation in the DeleteStateMachineVersion handler of the sfn service crate.
  Check: .delete-state-machine-version-validation-guard.

Cause 7 of 10: CreateStateMachine's oversized-definition ValidationException echoes the rejected definition into the message and names the wrong constraint.
  AWS: Smithy CreateStateMachine input member `definition` targets Definition with @length max 1048576 and is @sensitive. AWS's message is `1 validation error detected: Value at 'definition' failed to satisfy constraint: Member must have length less than or equal to 1048576` — the value is NOT echoed, precisely because the member is sensitive.
  Emulator: `1 validation error detected: Value '{"Comment": "xxxx...[1049161 bytes]' at 'definition' failed to satisfy constraint: Member must have length less than or equal to 1048576` — it interpolates a prefix of the sensitive definition. (This is the same defect class the open issue 20261024T043150Z-sfn-step-functions-control-plane-no-iam-passrole-authorization.md records for StartExecution's `input`; this is the `definition` member and is not carried by that issue.)
  Why: a sensitive member is leaked into an error message and the message no longer matches AWS's template.
  Fix: the constraint-violation message builder used by the CreateStateMachine handler of the sfn service crate — omit the value for @sensitive members.
  Check: .create-state-machine-definition-exceeds-size-limit-rejected.

Cause 8 of 10: InvalidDefinition messages are emulator prose rather than AWS's validator output.
  AWS: Smithy models InvalidDefinition (400) on CreateStateMachine/UpdateStateMachine/TestState. AWS reports `Invalid State Machine Definition: '<SEMICOLON-separated reason list>'` where each reason is a validator code plus its text and a JSON pointer, e.g. `SCHEMA_VALIDATION_FAILED: These fields are required: [States, StartAt] at /, SCHEMA_VALIDATION_FAILED: Field 'invalid' is not supported at /`; for malformed JSON the reason is `INVALID_JSON_DESCRIPTION:` followed by the Jackson parser text including the `[Source: (String)"..."; line: L, column: C]` locator.
  Emulator: `Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: a state machine definition must carry a string 'StartAt' member at /StartAt, SCHEMA_VALIDATION_FAILED: a state machine definition must carry an object 'States' member at /States'` and `'INVALID_JSON_DESCRIPTION: the definition is not valid JSON: Unexpected end-of-input: expected close marker for Object'` — different reason text, different pointers, and no source locator.
  Fix: the ASL validator's diagnostic formatting in the sfn service crate.
  Checks: .error-invalid-definition, .test-state-invalid-definition-rejected.

Cause 9 of 10: TestState's InvalidExecutionInput message for malformed input JSON.
  AWS: Smithy TestState errors include InvalidExecutionInput (400); AWS's message carries the parser text with the `[Source: (String)"..."; line: L, column: C]` locator for the submitted input.
  Emulator: `Invalid State Machine Execution Input: 'Unexpected end-of-input: expected close marker for Object'` — no locator.
  Fix: input parsing diagnostics in the TestState handler of the sfn service crate.
  Check: .test-state-invalid-execution-input-rejected.

Cause 10 of 10: UpdateStateMachine with no updatable member names only two of the four members AWS names.
  AWS: Smithy UpdateStateMachine errors include MissingRequiredParameter (400); AWS's message is the fixed text `Either the definition, the role ARN, the LoggingConfiguration, or the TracingConfiguration must be specified`.
  Emulator: `either definition or roleArn must be given to update a state machine` — wrong text, and it mis-states the rule by omitting loggingConfiguration and tracingConfiguration, both of which the model accepts as sole updates.
  Fix: the required-member check in the UpdateStateMachine handler of the sfn service crate.
  Checks: .update-state-machine-missing-required-parameter, .update-state-machine-missing-required-parameter-fails.

## Evidence
Cause 1: `AssertionError: expected message "Resource not found: '<arn>'", got 'could not find the resource <arn>'` (10 checks; the ARN is a per-run value, the template is the requirement).
Cause 2: `AssertionError: unexpected ConflictException message "State Machine Alias Already Exists: '<alias arn>'"` — expected `Failed to create alias because an alias with the same name and a different routing configuration already exists.`
Cause 3: `AssertionError: expected ValidationException for a purely numeric alias name, got InvalidName`; `AssertionError: expected ValidationException, got InvalidName`.
Cause 4: `AssertionError: unexpected ValidationException message 'the weights of a routing configuration must add up to 100, got 42'` — expected `Sum of routing configuration weights must equal 100. Received: [42]`.
Cause 5: `AssertionError: expected message 'Version to be deleted must not be referenced by an alias. Current list of aliases referencing this version: [<alias name>]', got "'<version arn>' is referenced by the state machine alias(es) <alias arn>, which must be deleted or pointed at another version before it can be deleted"`.
Cause 6: `AssertionError: expected message 'Arn missing version qualifier: <arn>', got "Invalid Arn: 'Invalid ARN stateMachine version: <arn>'"`.
Cause 7: `AssertionError: unexpected ValidationException message '1 validation error detected: Value \'{"Comment": "xxxx…[1049161 bytes]\' at \'definition\' failed to satisfy constraint: Member must have length less than or equal to 1048576'` — expected `1 validation error detected: Value at 'definition' failed to satisfy constraint: Member must have length less than or equal to 1048576`.
Cause 8: `AssertionError: expected message "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: These fields are required: [States, StartAt] at /, SCHEMA_VALIDATION_FAILED: Field 'invalid' is not supported at /'", got "Invalid State Machine Definition: 'SCHEMA_VALIDATION_FAILED: a state machine definition must carry a string 'StartAt' member at /StartAt, ...'"`.
Cause 9: `AssertionError: unexpected InvalidExecutionInput message: "Invalid State Machine Execution Input: 'Unexpected end-of-input: expected close marker for Object'"`.
Cause 10: `AssertionError: expected message 'Either the definition, the role ARN, the LoggingConfiguration, or the TracingConfiguration must be specified', got 'either definition or roleArn must be given to update a state machine'`.

## Steps to Reproduce
Cause 1: DescribeStateMachineAlias / DeleteStateMachineAlias / DescribeMapRun / UpdateMapRun / UntagResource on an ARN that does not exist and read Error.Message.
Cause 2: CreateStateMachineAlias twice with the same name and different routingConfiguration.
Cause 3: CreateStateMachineAlias with name="12345" (fails the modelled pattern) and read Error.Code.
Cause 4: CreateStateMachineAlias/UpdateStateMachineAlias with routingConfiguration weights summing to 42.
Cause 5: publish a version, point an alias at it, then DeleteStateMachineVersion on that version.
Cause 6: DeleteStateMachineVersion with an unqualified state-machine ARN.
Cause 7: CreateStateMachine with a well-formed definition longer than 1048576 characters (client-side parameter validation disabled).
Cause 8: CreateStateMachine with `{"invalid": true}` and TestState with a definition missing a closing brace.
Cause 9: TestState with input='{"probe": 1'.
Cause 10: UpdateStateMachine with only stateMachineArn.
