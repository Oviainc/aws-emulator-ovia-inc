Issue-Source: trace-sweep-divergence
Issue-Trace: sfn-encryption-and-logging-config-validation
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sfn-encryption-and-logging-config-validation states-state-machine-lifecycle stepfunctions-state-machine-crud-and-tagging states-express-state-machine-type-restrictions states-state-machine-lifecycle-crud
Issue-Shape-Id: com.amazonaws.sfn#AWSStepFunctions
Issue-Key: states|CreateStateMachine|4xx->2xx|$.__type
Issue-Key: states|CreateStateMachine|4xx->2xx|$.message
Issue-Key: states|CreateStateMachine|4xx->2xx|status
Issue-Key: states|CreateStateMachine|2xx->4xx|$.creationDate
Issue-Key: states|CreateStateMachine|2xx->4xx|$.stateMachineArn
Issue-Key: states|CreateStateMachine|2xx->4xx|$.stateMachineVersionArn
Issue-Key: states|CreateStateMachine|2xx->4xx|status
Issue-Commit: d2e37a828af0176472a4a50a2ff1b55038dac16f

# CreateStateMachine skips encryption-configuration validation and refuses creations AWS accepts

> _Note: this issue was identified against the code base at commit `d2e37a828af0176472a4a50a2ff1b55038dac16f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStateMachine

## Description
CreateStateMachine gets the accept/reject decision wrong in both directions: an invalid encryptionConfiguration is accepted, and two creations AWS accepts are refused with 400. 3 causes.

Cause 1 of 3: an invalid encryptionConfiguration is accepted with 200 instead of rejected.
AWS: Smithy: CreateStateMachine errors include InvalidEncryptionConfiguration (client, HTTP 400), documented as 'Received when encryptionConfiguration is specified but various conditions exist which make the configuration invalid. For example, if type is set to CUSTOMER_MANAGED_KMS_KEY, but kmsKeyId is null, or kmsDataKeyReusePeriodSeconds is not between 60 and 900...' (Smithy: EncryptionConfiguration.type required, kmsKeyId length 1..2048, kmsDataKeyReusePeriodSeconds range 60..900). The recording answers HTTP 400 with __type 'com.amazonaws.swf.service.v2.model#InvalidEncryptionConfiguration' and message 'Invalid Encryption Configuration: Must set a valid kmsKeyId.' (example text; the rule is that CUSTOMER_MANAGED_KMS_KEY without a usable kmsKeyId is invalid).
Emulator: answers HTTP 200 and creates the state machine, with no __type and no message.
Why: an invalid encryption setup is silently accepted, so a client believes its data is encrypted under a customer-managed key that does not exist, and negative tests for this error never fire.
Fix: CreateStateMachine input validation in the sfn service (encryptionConfiguration consistency check raising InvalidEncryptionConfiguration before the state machine is stored).
Traces: sfn-encryption-and-logging-config-validation#task#3 (also sfn-encryption-and-logging-config-validation#task#4/#5, states-state-machine-lifecycle#task#7, stepfunctions-state-machine-crud-and-tagging#task#11).

Cause 2 of 3: a published EXPRESS state machine creation AWS accepts is refused with 400.
AWS: Smithy: CreateStateMachine output stateMachineArn (required), creationDate (required), stateMachineVersionArn. The recording answers HTTP 200 with stateMachineArn 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-probe-stepfunctions-sm-express-bad-20b8d043', the matching ':1' stateMachineVersionArn (publish=true) and a creationDate.
Emulator: answers HTTP 400 and returns no stateMachineArn/stateMachineVersionArn/creationDate. Hypothesis: the handler's definition or type validation rejects a definition AWS accepts at create time (AWS validates only structural requirements at CreateStateMachine and defers the rest to execution).
Why: the fixture's state machine cannot be created at all, so everything downstream of it is unreachable; clients see a spurious client error for a definition AWS accepts.
Fix: CreateStateMachine validation in the sfn service - narrow the create-time definition/type checks to what AWS rejects, and emit the version ARN when publish is set.
Traces: states-express-state-machine-type-restrictions#task#9.

Cause 3 of 3: a second standard state machine creation AWS accepts is refused with 400.
AWS: the recording answers HTTP 200 with stateMachineArn 'arn:aws:states:us-west-2:000000000000:stateMachine:sim2real-probe-stepfunctions-sm-crud-e48bec84' and a creationDate.
Emulator: answers HTTP 400 with none of those members. Hypothesis: the same over-strict create-time validation path as cause 2 (no conflicting name is expected at this point in the fixture).
Why: a valid creation fails, so CRUD flows cannot be exercised.
Fix: same CreateStateMachine validation path.
Traces: states-state-machine-lifecycle-crud#task#7.

## Evidence
- sfn-encryption-and-logging-config-validation#task#3: expected status 400 with $.__type='com.amazonaws.swf.service.v2.model#InvalidEncryptionConfiguration' and $.message='Invalid Encryption Configuration: Must set a valid kmsKeyId.'; actual status 200 with neither member (5 affected steps).
- states-express-state-machine-type-restrictions#task#9: expected status 200 with $.stateMachineArn=...:stateMachine:sim2real-probe-stepfunctions-sm-express-bad-20b8d043, $.stateMachineVersionArn=...:1 and $.creationDate; actual status 400, members absent.
- states-state-machine-lifecycle-crud#task#7: expected status 200 with $.stateMachineArn=...:stateMachine:sim2real-probe-stepfunctions-sm-crud-e48bec84 and $.creationDate; actual status 400, members absent.

## Steps to Reproduce
1. Replay fixture sfn-encryption-and-logging-config-validation step task#3: CreateStateMachine with encryptionConfiguration type=CUSTOMER_MANAGED_KMS_KEY and no usable kmsKeyId -> emulator answers 200 and creates the machine instead of 400 InvalidEncryptionConfiguration.
2. Replay states-express-state-machine-type-restrictions step task#9 and states-state-machine-lifecycle-crud step task#7: CreateStateMachine calls the recording answers 200 -> emulator answers 400.
