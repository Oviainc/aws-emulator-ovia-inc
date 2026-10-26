Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event
Issue-Key: events__lambda__rds__sqs|CreateDBParameterGroup|NotImplemented
Issue-Commit: 6f030629c911e30ed6652430413113629aa7dccf

# RDS CreateDBParameterGroup is an unimplemented scaffold stub, so no DB instance can be stood up and the EventBridge Lambda-target dead-letter path for RDS events is unreachable (1 cause)

> _Note: this issue was identified against the code base at commit `6f030629c911e30ed6652430413113629aa7dccf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBParameterGroup

## Description
Cause 1 of 1 — RDS `CreateDBParameterGroup` answers NotImplemented.
AWS: `CreateDBParameterGroup` takes DBParameterGroupName, DBParameterGroupFamily and Description (all required) plus Tags, and returns the DBParameterGroup with its Name, Family, Description and DBParameterGroupArn; its modeled client errors are DBParameterGroupAlreadyExistsFault and DBParameterGroupQuotaExceededFault. It is a prerequisite for creating a DB instance with non-default parameters.
Emulator: answers HTTP 400 with error code `NotImplemented` and "operation CreateDBParameterGroup is scaffolded but not implemented in this emulator". `NotImplemented` is not a modeled error for this operation, and the RDS query protocol surfaces it as the request's Error code.
Why: the parameter group is the first step of the check's provisioning, so no DB instance exists, no `aws.rds` event is produced, and the behaviour actually under test — an EventBridge rule whose Lambda target lacks lambda:InvokeFunction must DEAD-LETTER the RDS event rather than drop it — cannot be reached. The client also gets an error code that appears in no RDS API reference.
Fix: the CreateDBParameterGroup handler, services/service-com-amazonaws-rds-amazonrdsv19/src/ops/create_db_parameter_group.rs.
Traces: events__lambda__rds__sqs.xsvc-rds-3-eventbridge-lambda-target-without-invoke-permission-dead-letters-rds-event.

## Evidence
"botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateDBParameterGroup operation: operation CreateDBParameterGroup is scaffolded but not implemented in this emulator". Expected: HTTP 200 with DBParameterGroup{DBParameterGroupName, DBParameterGroupFamily, Description, DBParameterGroupArn}.

## Steps to Reproduce
1. CreateDBParameterGroup(DBParameterGroupName=g, DBParameterGroupFamily='postgres16', Description='d') -> HTTP 400 with code NotImplemented instead of 200 with the DBParameterGroup.
2. The follow-on CreateDBInstance, the `aws.rds` event it emits, and the EventBridge rule whose Lambda target lacks invoke permission (which must dead-letter) are all unreachable.
