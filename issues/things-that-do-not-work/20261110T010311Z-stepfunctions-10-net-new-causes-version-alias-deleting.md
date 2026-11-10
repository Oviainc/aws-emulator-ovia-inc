Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: stepfunctions.start-sync-execution-nonexistent-version-and-alias-rejected stepfunctions.start-sync-execution-on-deleted-version-and-alias-arn-does-not-exist stepfunctions.list-executions-filtered-by-version-and-alias-arn stepfunctions.alias-and-version-mutations-rejected-while-state-machine-deleting stepfunctions.redrive-and-express-sync-start-rejected-while-state-machine-deleting stepfunctions.start-sync-execution-on-deleting-state-machine-rejected stepfunctions.create-activity-already-exists-omitting-cmk stepfunctions.create-activity-nonexistent-key-arn-invalid-encryption-config-rejected stepfunctions.create-state-machine-already-exists-publish-state-mismatch stepfunctions.publish-version-qualified-arn-rejected stepfunctions.update-state-machine-alias-no-fields-rejected stepfunctions.update-state-machine-no-op-and-publish-idempotency stepfunctions.update-state-machine-publish-at-version-cap-quota-exceeded stepfunctions.test-state-assign-rejected-on-succeed-and-fail stepfunctions.test-state-map-type-rejected stepfunctions.test-state-schema-validation-rejects-unsupported-fields stepfunctions.test-state-invalid-variables-rejected stepfunctions.test-state-task-unassumable-role stepfunctions.describe-state-machine-for-execution-labeled-arn-ignores-metadata-only
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: stepfunctions.start-sync-execution-nonexistent-version-and-alias-rejected|run#17|AssertionError|assert version_error.get("Message") == expected_version_message, (
Issue-Key: stepfunctions.start-sync-execution-on-deleted-version-and-alias-arn-does-not-exist|run#16|AssertionError|assert version_error.get("Message") == expected_version_message, (
Issue-Key: stepfunctions|ListExecutions|StateMachineDoesNotExist
Issue-Key: stepfunctions.alias-and-version-mutations-rejected-while-state-machine-deleting|run#17|AssertionError|assert err == {"Code": "StateMachineDeleting", "Message": expected}, (
Issue-Key: stepfunctions.redrive-and-express-sync-start-rejected-while-state-machine-deleting|run#12|AssertionError|assert err == {"Code": "ExecutionNotRedrivable", "Message": expected}, (
Issue-Key: stepfunctions.start-sync-execution-on-deleting-state-machine-rejected|_poll_until_stable_error#8|AssertionError|raise AssertionError(
Issue-Key: stepfunctions.create-activity-already-exists-omitting-cmk|run#12|AssertionError|assert baseline.get("type") == "CUSTOMER_MANAGED_KMS_KEY" and baseline.get("kmsKeyId") == key_arn, (
Issue-Key: stepfunctions.create-activity-nonexistent-key-arn-invalid-encryption-config-rejected|_error#4|AssertionError|raise AssertionError(f"expected the call to be rejected, but it succeeded: {result!r}")
Issue-Key: stepfunctions.create-state-machine-already-exists-publish-state-mismatch|run#11|AssertionError|assert message == (
Issue-Key: stepfunctions.publish-version-qualified-arn-rejected|run#10|AssertionError|assert version_code == "InvalidArn", f"expected InvalidArn on a version-qualified ARN, got {version_code!r}"
Issue-Key: stepfunctions.update-state-machine-alias-no-fields-rejected|run#12|AssertionError|assert err == expected, f"UpdateStateMachineAlias with neither field: expected {expected}, got {err}"
Issue-Key: stepfunctions.update-state-machine-no-op-and-publish-idempotency|run#7|AssertionError|assert noop["updateDate"] == created["creationDate"], (
Issue-Key: stepfunctions.update-state-machine-publish-at-version-cap-quota-exceeded|run#15|AssertionError|assert message == expected_message, f"expected message {expected_message!r}, got {message!r}"
Issue-Key: stepfunctions.test-state-assign-rejected-on-succeed-and-fail|_error#3|AssertionError|raise AssertionError(f"expected the definition to be rejected, but it succeeded and returned {result!r}")
Issue-Key: stepfunctions.test-state-map-type-rejected|run#9|AssertionError|assert err == expected, f"TestState on a Map-type definition: expected {expected}, got {err}"
Issue-Key: stepfunctions.test-state-schema-validation-rejects-unsupported-fields|run#7|AssertionError|assert message == (
Issue-Key: stepfunctions.test-state-invalid-variables-rejected|run#5|AssertionError|assert message == "The value of 'variables' is not valid JSON.", (
Issue-Key: stepfunctions.test-state-task-unassumable-role|run#5|AssertionError|assert control["status"] == "SUCCEEDED", f"positive control did not succeed: {control!r}"
Issue-Key: stepfunctions.describe-state-machine-for-execution-labeled-arn-ignores-metadata-only|run#28|AssertionError|assert child_meta.get("variableReferences") == expected_refs, (
Issue-Commit: 4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666

# stepfunctions: 10 net-new causes — version/alias/DELETING refusals, activity CMK, quota and idempotency messages, and TestState behaviour

> _Note: this issue was identified against the code base at commit `4d67debdc5b6a4a4b59f91b6ecdc4b307fee9666`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSyncExecution

## Description
Ten independent root causes in the control plane's lookup/refusal paths, resource state handling and TestState.

Cause 1 of 10: an unknown version or alias qualifier is reported with an invented message.
AWS: an operation on a version- or alias-qualified ARN that does not exist is refused StateMachineDoesNotExist with message `State Machine Does Not Exist: '<the qualified ARN as submitted>'` — fixed prefix plus the submitted ARN. Smithy: StateMachineDoesNotExist is modelled on StartSyncExecution and ListExecutions, httpError 400, documented "The specified state machine does not exist."
Emulator: `State Machine Version Does Not Exist: '<arn>'` (an extra word AWS does not use), on StartSyncExecution for a nonexistent and for a deleted version/alias, and on ListExecutions filtered by a version ARN — where it surfaces as an unhandled StateMachineDoesNotExist.
Why: clients matching the documented message do not match, and the emulator invents an error-message family for qualifiers.
Fix: the not-found message in the qualified-ARN resolution shared by these handlers.
Traces: stepfunctions.start-sync-execution-nonexistent-version-and-alias-rejected, stepfunctions.start-sync-execution-on-deleted-version-and-alias-arn-does-not-exist, stepfunctions.list-executions-filtered-by-version-and-alias-arn

Cause 2 of 10: refusals while a state machine is DELETING carry the wrong code or wording.
AWS: CreateStateMachineAlias against a deleting state machine is refused with message `Failed to create alias because the State Machine <name> is in the DELETING state`; RedriveExecution is refused ExecutionNotRedrivable with reason `Execution <execution ARN> invalid for redrive as: State machine is DELETING`; and StartSyncExecution on a deleting state machine is refused StateMachineDeleting (Smithy: StateMachineDeleting is modelled on CreateStateMachineAlias and StartSyncExecution, httpError 400, "The specified state machine is being deleted."), stably, for as long as it is deleting.
Emulator: returns `{'Code': 'StateMachineDeleting', 'Message': "State Machine is being deleted: '<arn>'"}` for the alias case (wrong message), `... invalid for redrive as: State machine is in DELETING status` (extra words) for redrive, and for StartSyncExecution flips to `{'Code': 'StateMachineDoesNotExist', 'Message': "State Machine Does Not Exist: '<arn>'"}` so it never settles on StateMachineDeleting.
Why: a client cannot distinguish "being deleted" from "never existed", and retry logic keyed on the documented message/reason misfires.
Fix: the DELETING-state precondition and its messages in the alias/redrive/start-sync handlers.
Traces: stepfunctions.alias-and-version-mutations-rejected-while-state-machine-deleting, stepfunctions.redrive-and-express-sync-start-rejected-while-state-machine-deleting, stepfunctions.start-sync-execution-on-deleting-state-machine-rejected

Cause 3 of 10: CreateActivity ignores encryptionConfiguration and accepts a nonexistent CMK.
AWS: CreateActivity stores the `encryptionConfiguration` it is given (DescribeActivity reports `{'kmsKeyId': ..., 'kmsDataKeyReusePeriodSeconds': 300, 'type': 'CUSTOMER_MANAGED_KMS_KEY'}`), and refuses InvalidEncryptionConfiguration when the kmsKeyId names a key that does not exist. Smithy: CreateActivity.encryptionConfiguration, errors include InvalidEncryptionConfiguration (400).
Emulator: the activity is created without its encryptionConfiguration, and a nonexistent key ARN is accepted with HTTP 200.
Why: a CMK-encrypted activity is silently unencrypted, and a typo'd key ARN is never caught.
Fix: encryptionConfiguration persistence and key validation in the CreateActivity handler.
Traces: stepfunctions.create-activity-already-exists-omitting-cmk, stepfunctions.create-activity-nonexistent-key-arn-invalid-encryption-config-rejected

Cause 4 of 10: the StateMachineAlreadyExists message is invented.
AWS: re-creating a state machine with the same name but a different definition or role is refused StateMachineAlreadyExists with AWS's documented message (Smithy: "A state machine with the same name but a different definition or role ARN already exists."), naming the state machine.
Emulator: `State Machine Already Exists: '<arn>'`, which the check rejects as not AWS's text; the publish state of the pre-existing machine is also mis-reported.
Fix: the conflict message in CreateStateMachine's already-exists path.
Traces: stepfunctions.create-state-machine-already-exists-publish-state-mismatch

Cause 5 of 10: PublishStateMachineVersion on a qualified ARN is ValidationException instead of InvalidArn.
AWS: a version- or alias-qualified stateMachineArn is refused InvalidArn. Smithy: PublishStateMachineVersion models both InvalidArn (400) and ValidationException (400); the qualified-ARN case is InvalidArn ("The provided Amazon Resource Name (ARN) is not valid.").
Emulator: ValidationException.
Fix: the ARN-shape check in the PublishStateMachineVersion handler.
Traces: stepfunctions.publish-version-qualified-arn-rejected

Cause 6 of 10: UpdateStateMachineAlias with neither field has the wrong message.
AWS: ValidationException with message `Either description or routing configuration must be specified` (fixed text).
Emulator: `At least one of the description or routingConfiguration parameters must be specified`.
Fix: the message in the UpdateStateMachineAlias precondition.
Traces: stepfunctions.update-state-machine-alias-no-fields-rejected

Cause 7 of 10: a no-op UpdateStateMachine moves updateDate.
AWS: an UpdateStateMachine that changes nothing leaves the state machine's updateDate equal to its creationDate (publish stays idempotent).
Emulator: updateDate advances past creationDate on a no-op update.
Why: clients that detect real changes by comparing the two timestamps see a change that did not happen.
Fix: the no-op short-circuit in the UpdateStateMachine handler / state.
Traces: stepfunctions.update-state-machine-no-op-and-publish-idempotency

Cause 8 of 10: the version cap is reported as an emulator storage limit.
AWS: publishing past the per-state-machine version cap is refused with message `Limit on maximum number of versions for state machine <state machine name> reached` (fixed text plus the machine's name). Smithy: ServiceQuotaExceededException (402) is modelled on UpdateStateMachine and PublishStateMachineVersion.
Emulator: `publishing a version would take the store past the 67108864 bytes this emulator retains` — an emulator-internal byte budget, not a quota AWS has.
Fix: the version-cap accounting and message in the publish path.
Traces: stepfunctions.update-state-machine-publish-at-version-cap-quota-exceeded

Cause 9 of 10: TestState diverges on validation and on running a Task.
AWS: TestState refuses a definition that carries Assign on a Succeed or Fail state; refuses a Map/Parallel definition with `{'Code': 'InvalidDefinition', 'Status': 400, 'Message': 'TestState API does not support Map or Parallel states. Supported state types include: [Task, Wait, Pass, Succeed, Fail, Choice]'}`; words its unsupported-field diagnostic as AWS does; rejects malformed `variables` with AWS's message (never a Jackson parser dump); and actually performs a Task state's integration call under the supplied roleArn, so an unassumable role is what fails — the positive control succeeds. Smithy: TestState errors are InvalidArn, InvalidDefinition, InvalidExecutionInput, ValidationException.
Emulator: accepts Assign on Succeed/Fail (returns `{'output': '{}', 'status': 'SUCCEEDED'}`); answers the Map case `{'Code': 'ValidationException', 'Message': "a 'Map' state is tested only when a mock is specified for its outcome"}`; emits `SCHEMA_VALIDATION_FAILED: Field 'Resource' is not supported`; leaks `Unexpected end-of-input: expected close marker for Object (start marker at [Source: REDACTED (\`StreamReadFeature.INCLUDE_SOURCE_IN_LOCATION\` disabled); line: 1, column: 1])` for malformed variables; and never invokes a Task, failing the positive control with States.Runtime cause `a 'Task' state's own work is an invocation this emulator does not make, and the request names no mock to stand in for it, so nothing was run for it`.
Why: TestState — the API whose whole purpose is pre-flight state testing — reports wrong codes/messages and cannot test a Task at all.
Fix: the TestState handler: definition validation, the Map/Parallel refusal, the variables parser error, and Task invocation.
Traces: stepfunctions.test-state-assign-rejected-on-succeed-and-fail, stepfunctions.test-state-map-type-rejected, stepfunctions.test-state-schema-validation-rejects-unsupported-fields, stepfunctions.test-state-invalid-variables-rejected, stepfunctions.test-state-task-unassumable-role

Cause 10 of 10: DescribeStateMachineForExecution drops variableReferences for a labeled / METADATA_ONLY execution.
AWS: the response still carries `variableReferences` (e.g. `{'Use': ['w']}`) even when includedData is METADATA_ONLY, because variable references are metadata, not execution data.
Emulator: returns None for variableReferences.
Fix: the DescribeStateMachineForExecution serializer's handling of includedData.
Traces: stepfunctions.describe-state-machine-for-execution-labeled-arn-ignores-metadata-only

## Evidence
1) expected `State Machine Does Not Exist: 'arn:...:stateMachine:<name>:999'`; got `State Machine Version Does Not Exist: 'arn:...:<name>:999'` (also raised out of ListExecutions as `botocore.errorfactory.StateMachineDoesNotExist`).
2) expected `Failed to create alias because the State Machine <name> is in the DELETING state`; got `{'Code': 'StateMachineDeleting', 'Message': "State Machine is being deleted: '<arn>'"}`. Expected `... invalid for redrive as: State machine is DELETING`; got `... is in DELETING status`. StartSyncExecution: `never settled on a stable StateMachineDeleting within 120.0s; last={'Message': "State Machine Does Not Exist: '<arn>'", 'Code': 'StateMachineDoesNotExist'}`.
3) `the activity was not created with its CMK encryptionConfiguration ({'kmsKeyId': ..., 'kmsDataKeyReusePeriodSeconds': 300, 'type': 'CUSTOMER_MANAGED_KMS_KEY'})`; `expected the call to be rejected, but it succeeded: {'activityArn': '...nokeyarn...', 'HTTPStatusCode': 200}`.
4) `true-then-false: unexpected message "State Machine Already Exists: '<arn>'"`.
5) `expected InvalidArn on a version-qualified ARN, got 'ValidationException'`.
6) expected `Either description or routing configuration must be specified`; got `At least one of the description or routingConfiguration parameters must be specified`.
7) `an update with unchanged config must report updateDate == creationDate, got updateDate=...0:42:22.285 creationDate=...0:42:22.276`.
8) expected `Limit on maximum number of versions for state machine <name> reached`; got `publishing a version would take the store past the 67108864 bytes this emulator retains`.
9) see the message pairs quoted in Cause 9 (TestState Map case, SCHEMA_VALIDATION_FAILED text, Jackson dump, and the States.Runtime positive-control failure).
10) `expected METADATA_ONLY on the labeled execution to still carry variableReferences {'Use': ['w']}, got None`.

## Steps to Reproduce
1. StartSyncExecution / ListExecutions against `<sm-arn>:999` and against a deleted version and alias ARN.
2. DeleteStateMachine, then (while DELETING) CreateStateMachineAlias, RedriveExecution of a failed execution, and StartSyncExecution.
3. CreateActivity with encryptionConfiguration for a real CMK, then DescribeActivity; CreateActivity with a kmsKeyId that does not exist.
4. CreateStateMachine twice with the same name and different definitions.
5. PublishStateMachineVersion with `<sm-arn>:1`.
6. UpdateStateMachineAlias with neither description nor routingConfiguration.
7. UpdateStateMachine with exactly the current definition and role; compare updateDate to creationDate.
8. Publish versions until the cap is reached.
9. TestState with: Assign on a Succeed state; a Map-type definition; an unsupported field; malformed `variables`; a Task with a roleArn that cannot be assumed (plus the assumable positive control).
10. DescribeStateMachineForExecution on a labeled execution ARN with includedData=METADATA_ONLY.
