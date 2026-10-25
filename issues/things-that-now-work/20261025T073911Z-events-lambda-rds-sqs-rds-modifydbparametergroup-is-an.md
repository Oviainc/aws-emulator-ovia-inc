Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event
Issue-Key: events__lambda__rds__sqs|ModifyDBParameterGroup|NotImplemented
Issue-Commit: b0d906bf2f6d2f0ea5fb379cb763f1de287c532e

# events__lambda__rds__sqs: RDS ModifyDBParameterGroup is an unimplemented scaffold stub, so the RDS API call that must produce the aws.rds event can never be made

> _Note: this issue was identified against the code base at commit `b0d906bf2f6d2f0ea5fb379cb763f1de287c532e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ModifyDBParameterGroup

## Description
Cause 1 of 1 - RDS `ModifyDBParameterGroup` answers NotImplemented, so the RDS management call the whole EventBridge dead-letter scenario is driven by cannot be issued.

AWS: Smithy (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json) models ModifyDBParameterGroup with required DBParameterGroupName and a required Parameters list (ParameterName/ParameterValue/ApplyMethod, max 20 per request) and returns DBParameterGroupNameMessage{DBParameterGroupName} on HTTP 200. Its only modeled client errors are DBParameterGroupNotFoundFault (404, awsQueryError code DBParameterGroupNotFound) and InvalidDBParameterGroupStateFault (400, awsQueryError code InvalidDBParameterGroupState). `NotImplemented` is not a modeled error for this operation and appears in no RDS API reference.

Emulator: the request is refused with error code `NotImplemented` and the message "operation ModifyDBParameterGroup is scaffolded but not implemented in this emulator". services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_db_parameter_group.rs still carries the `SCAFFOLDED, NOT IMPLEMENTED` marker and its handle() body is `Err(crate::scaffolded::Scaffolded::not_implemented("ModifyDBParameterGroup"))`, so every call is refused regardless of input.

Why: a client cannot change any DB parameter group parameter at all, and it receives an error code that is not in the operation's modeled error set, so error-handling branches written against DBParameterGroupNotFoundFault / InvalidDBParameterGroupStateFault never match. Downstream, the behavior actually under test is unreachable: this ModifyDBParameterGroup call is the RDS management event the EventBridge rule matches, so the rule never fires and the assertion that a Lambda target without a lambda:InvokeFunction resource policy for events.amazonaws.com must DEAD-LETTER the event (ERROR_CODE=NO_PERMISSIONS, ERROR_MESSAGE, RULE_ARN, TARGET_ARN, original event body) can never be graded.

Fix: the ModifyDBParameterGroup handler, services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_db_parameter_group.rs (implement handle(), drop the SCAFFOLDED marker line). This is a DIFFERENT operation from the already-open 20261025T002357Z-events-lambda-rds-sqs-rds-createdbparametergroup-is-an-unimplemented-scaffold-stub-so.md, which covers CreateDBParameterGroup for the same check; that call now gets past the stub and the replay stops one operation later, here.

Traces: events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event.

## Evidence
Failing check events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event, verdict error:

botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ModifyDBParameterGroup operation: operation ModifyDBParameterGroup is scaffolded but not implemented in this emulator

Expected (AWS): HTTP 200 with DBParameterGroupNameMessage{DBParameterGroupName: <group>}; a bad name is refused 404 DBParameterGroupNotFound and a bad group state 400 InvalidDBParameterGroupState.
Actual (emulator): a NotImplemented refusal from the scaffold stub (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/modify_db_parameter_group.rs returns Scaffolded::not_implemented unconditionally), an error code the operation does not model.

## Steps to Reproduce
1. CreateDBParameterGroup(DBParameterGroupName=g, DBParameterGroupFamily=<family>, Description=d).
2. ModifyDBParameterGroup(DBParameterGroupName=g, Parameters=[{ParameterName: <dynamic param>, ParameterValue: <value>, ApplyMethod: 'immediate'}]).
   - AWS: HTTP 200 with {DBParameterGroupName: g}.
   - Emulator: refused with error code NotImplemented ("operation ModifyDBParameterGroup is scaffolded but not implemented in this emulator").
3. Because step 2 never completes, no aws.rds management event is produced, the EventBridge rule matching it never delivers, and the Lambda-target-without-invoke-permission dead-letter assertions (ERROR_CODE=NO_PERMISSIONS on the target's SQS DLQ, function never invoked) cannot be reached.
