Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.termination-protection-disable-then-delete cloudformation.update-stack-adds-then-removes-a-real-queue cloudformation.ec2mr-cfnres-absent-stack-name-rejected-across-both-plural-ops cloudformation.token-already-exists-rejects-duplicate-request-token cloudformation.update-stack-duplicate-token-rejected cloudformation.force-delete-stack-rejected-unless-delete-failed cloudformation.type-not-found-error-across-apis cloudformation.type-registry-not-found-errors cloudformation.drift-detection-status-unknown-id cloudformation.list-pagination-stability cloudformation.changeset-duplicate-create-rejected cloudformation.update-use-previous-value-carries-the-noecho-parameter cloudformation.op-contract-validateTemplate cloudformation__sns__sqs.aws-prefixed-stack-tag-rejected-on-create-and-update
Issue-Key: cloudformation.termination-protection-disable-then-delete|run#23|AssertionError|assert gone == expected_gone, f"expected {expected_gone!r} mutating a deleted stack, got {gone!r}"
Issue-Key: cloudformation.update-stack-adds-then-removes-a-real-queue|_assert_stack_gone#32|AssertionError|assert refused.response["Error"]["Message"] == expected_message, (
Issue-Key: cloudformation.ec2mr-cfnres-absent-stack-name-rejected-across-both-plural-ops|run#14|AssertionError|assert message == expected_message, (
Issue-Key: cloudformation.token-already-exists-rejects-duplicate-request-token|_error_code_and_message#10|AssertionError|raise AssertionError(f"expected a ClientError, call succeeded with: {result!r}")
Issue-Key: cloudformation.update-stack-duplicate-token-rejected|run#14|AssertionError|assert code == "TokenAlreadyExistsException", f"expected TokenAlreadyExistsException, got {code!r} ({msg!r})"
Issue-Key: cloudformation.force-delete-stack-rejected-unless-delete-failed|_error#9|AssertionError|raise AssertionError("call was expected to be refused but SUCCEEDED")
Issue-Key: cloudformation.type-not-found-error-across-apis|run#13|AssertionError|assert error == expected_not_found, (
Issue-Key: cloudformation|DescribeType|TypeNotFoundException
Issue-Key: cloudformation.drift-detection-status-unknown-id|run#14|AssertionError|assert err.get("Message") == expected_message, (
Issue-Key: cloudformation.list-pagination-stability|run#27|AssertionError|assert error1.get("Message") == expected_message, f"expected message {expected_message!r}, got {error1!r}"
Issue-Key: cloudformation.changeset-duplicate-create-rejected|run#10|AssertionError|assert dup_msg == (
Issue-Key: cloudformation.update-use-previous-value-carries-the-noecho-parameter|run#18|AssertionError|assert error == expected, (
Issue-Key: cloudformation.op-contract-validateTemplate|run#11|AssertionError|assert message == expected, f"expected ValidateTemplate refusal message {expected!r}, got {message!r}"
Issue-Key: cloudformation__sns__sqs.aws-prefixed-stack-tag-rejected-on-create-and-update|_error#4|AssertionError|raise AssertionError("expected the call to be refused, but it succeeded")
Issue-Commit: 461d317fc71f56ed8ab822edc66618d36d651bfa

# CloudFormation refusal taxonomy diverges: wrong not-found/validation message templates, duplicate ClientRequestToken not TokenAlreadyExistsException, ForceDeleteStack ungated, reserved aws: tag prefix accepted (10 causes)

> _Note: this issue was identified against the code base at commit `461d317fc71f56ed8ab822edc66618d36d651bfa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeStacks

## Description
This issue bundles 10 net-new root causes in CloudFormation's refusal surface. Causes 1 and 4-9 are message-template defects: in each case the error CODE matches AWS and only the message text diverges, but the message rule is known because AWS builds it from the request (it quotes the identifier the caller passed in a fixed template), so the TEMPLATE is the requirement and the recorded strings below are examples of it. Causes 2, 3 and 10 are code/status defects. The open issue 20261022T043551Z-cloudformation-stack-mutation-guards-not-enforced-capabilities-client.md covers the guard family for other check ids.

Cause 1 of 10: the stack-not-found message uses the wrong template.
AWS: ValidationError whose message is 'Stack [<name>] does not exist' for stack mutations/describes by name, and 'Stack with id <id> does not exist' only when an id was supplied; DescribeStackResource(s) say "Stack '<name>' does not exist".
Emulator: always 'Stack with id <name> does not exist'.
Why: callers that distinguish an unknown stack from a malformed request by message shape mis-classify the refusal; the recorded AWS text is reproduced from the request, so the template is a contract.
Fix: the not-found refusal builder shared by the stack handlers.
Traces: cloudformation.termination-protection-disable-then-delete, cloudformation.update-stack-adds-then-removes-a-real-queue, cloudformation.ec2mr-cfnres-absent-stack-name-rejected-across-both-plural-ops.

Cause 2 of 10: a reused ClientRequestToken is not refused.
AWS: Smithy: CreateStack/UpdateStack/RollbackStack/CancelUpdateStack all model TokenAlreadyExistsException (client, HTTP 400); replaying a token that is already in flight is refused with it.
Emulator: the duplicate call succeeds (HTTP 200), or is refused with the unrelated ValidationError 'Stack:<arn> is in UPDATE_IN_PROGRESS state and can not be updated.'
Why: idempotency on retry is the point of the token — a retried deploy executes twice.
Fix: record ClientRequestToken per stack operation and raise TokenAlreadyExistsException in the mutating handlers.
Traces: cloudformation.token-already-exists-rejects-duplicate-request-token, cloudformation.update-stack-duplicate-token-rejected.

Cause 3 of 10: ForceDeleteStack is not gated by stack status.
AWS: the force-delete form is accepted only from DELETE_FAILED and otherwise refused.
Emulator: the call succeeds from any status.
Fix: status gating in the DeleteStack handler.
Traces: cloudformation.force-delete-stack-rejected-unless-delete-failed.

Cause 4 of 10: TypeNotFoundException message template.
AWS: "The type '<TypeName>' cannot be found."
Emulator: 'Extension with identifier <TypeName> does not exist'.
Fix: the registry not-found refusal builder (DescribeType and the other registry read handlers).
Traces: cloudformation.type-not-found-error-across-apis, cloudformation.type-registry-not-found-errors.

Cause 5 of 10: drift-detection-id not-found message template.
AWS: 'Stack drift detection result [<id>] does not exist'.
Emulator: 'Drift detection with id <id> does not exist'.
Fix: DescribeStackDriftDetectionStatus refusal builder.
Traces: cloudformation.drift-detection-status-unknown-id.

Cause 6 of 10: invalid NextToken message template.
AWS: ValidationError '<token> is not a valid NextToken'.
Emulator: 'Invalid NextToken: <token>'.
Fix: the shared pagination-token validator.
Traces: cloudformation.list-pagination-stability.

Cause 7 of 10: the duplicate-change-set message names the wrong attribute.
AWS: AlreadyExistsException 'ChangeSet <name> cannot be created due to a mismatch with existing attribute Template'.
Emulator: the same template but with 'attribute TemplateBody' — it leaks the wire member name instead of the attribute AWS names.
Fix: the CreateChangeSet conflict refusal builder.
Traces: cloudformation.changeset-duplicate-create-rejected.

Cause 8 of 10: UsePreviousValue conflict message template.
AWS: ValidationError 'Invalid input for parameter key <Key>. Cannot specify usePreviousValue as true and non empty value for a parameter'.
Emulator: 'Parameters: [<Key>] cannot specify both a ParameterValue and UsePreviousValue'.
Fix: UpdateStack parameter validation refusal builder.
Traces: cloudformation.update-use-previous-value-carries-the-noecho-parameter.

Cause 9 of 10: ValidateTemplate's template-format message carries no parse position.
AWS: ValidationError 'Template format error: YAML not well-formed. (line 1, column 36)' — the message is derived from the submitted body and names where parsing failed.
Emulator: a generic 'Template format error: the template body is not well-formed JSON or YAML, or declares more than a template's documented maximum number of resources, outputs or parameters'.
Why: the position is the only actionable part of the refusal.
Fix: ValidateTemplate (and the shared template parser) must report the parser's line/column.
Traces: cloudformation.op-contract-validateTemplate.

Cause 10 of 10: stack tags using the reserved aws: prefix are accepted.
AWS: Smithy: Tag.Key notes 'Tags owned by Amazon Web Services have the reserved prefix: aws:'; CreateStack and UpdateStack refuse a caller-supplied aws:-prefixed tag key with ValidationError.
Emulator: the call succeeds on both create and update.
Fix: tag-key validation shared by CreateStack/UpdateStack.
Traces: cloudformation__sns__sqs.aws-prefixed-stack-tag-rejected-on-create-and-update.

## Evidence
Replayed suite (error codes matched, messages/outcomes diverged):
- termination-protection-disable-then-delete: expected ('ValidationError','Stack [<name>] does not exist'), got ('ValidationError','Stack with id <name> does not exist'); same shape in update-stack-adds-then-removes-a-real-queue and ec2mr-cfnres-absent-stack-name-rejected-across-both-plural-ops (AWS: "Stack '<name>' does not exist").
- token-already-exists-rejects-duplicate-request-token: "expected a ClientError, call succeeded with HTTPStatusCode 200"; update-stack-duplicate-token-rejected: "expected TokenAlreadyExistsException, got 'ValidationError'".
- force-delete-stack-rejected-unless-delete-failed: "call was expected to be refused but SUCCEEDED".
- type-not-found-error-across-apis / type-registry-not-found-errors: got ('TypeNotFoundException','Extension with identifier X does not exist') vs AWS ("The type 'X' cannot be found.").
- drift-detection-status-unknown-id: got 'Drift detection with id ... does not exist'.
- list-pagination-stability: got ('ValidationError','Invalid NextToken: <token>').
- changeset-duplicate-create-rejected: got '... mismatch with existing attribute TemplateBody'.
- update-use-previous-value-carries-the-noecho-parameter: got 'Parameters: [Carried] cannot specify both a ParameterValue and UsePreviousValue'.
- op-contract-validateTemplate: generic 'Template format error: the template body is not well-formed JSON or YAML, ...'.
- aws-prefixed-stack-tag-rejected-on-create-and-update: "expected the call to be refused, but it succeeded".

## Steps to Reproduce
1. Boot the emulator, replay the suite, re-run the trace_ids.
2. Cause 1 repro: DescribeStacks(StackName='no-such-stack') and UpdateStack on a deleted stack — compare the Error.Message template.
3. Cause 2 repro: UpdateStack twice with the same ClientRequestToken — the second call must answer TokenAlreadyExistsException (400).
4. Cause 10 repro: CreateStack with Tags=[{Key:'aws:mine',Value:'x'}] — AWS refuses with ValidationError.
