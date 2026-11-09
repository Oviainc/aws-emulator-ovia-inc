Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__scheduler.deploy-scheduler-schedule-malformed-rate-expression-rejected
Issue-Key: cloudformation__scheduler.deploy-scheduler-schedule-malformed-rate-expression-rejected|run#19|AssertionError|assert match is not None, f"CREATE_FAILED reason is not a resource-handler message: {reason!r}"
Issue-Commit: e2ad54ee7b8ee1b2faa65791f8cb738708e5249b

# CloudFormation CREATE_FAILED reason for a rejected AWS::Scheduler::Schedule drops the "Invalid request provided:" prefix and the whole (Service/Status Code/Request ID) clause

> _Note: this issue was identified against the code base at commit `e2ad54ee7b8ee1b2faa65791f8cb738708e5249b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
This issue carries 1 root cause, in the CloudFormation resource-handler error wrapping used when an AWS::Scheduler::Schedule resource is refused by EventBridge Scheduler.

Cause 1 of 1: the ResourceStatusReason CloudFormation records on the CREATE_FAILED row of a resource whose downstream service refused the call is not AWS's resource-handler message form.
AWS: when a resource provider's downstream AWS call fails, CloudFormation records the failure on the resource's CREATE_FAILED stack event as ResourceStatusReason in a fixed form (recorded behavior; the Smithy model for cloudformation types StackEvent.ResourceStatusReason only as a string, so the shape of the text is recorded behavior, not modelled):
  Resource handler returned message: "<the handler's message> (Service: <SdkId of the refusing service>, Status Code: <the refusing call's HTTP status>, Request ID: <request id>)" (RequestToken: <token>, HandlerErrorCode: <code>)
The fixed text is `Resource handler returned message: "`, ` (Service: `, `, Status Code: `, `, Request ID: `, `)" (RequestToken: `, `, HandlerErrorCode: ` and the trailing `)`. The variable parts are: the handler message, which for a Scheduler validation refusal is `Invalid request provided: ` followed verbatim by the Scheduler ValidationException message (so for a rejected rate expression: `Invalid request provided: Invalid Schedule Expression <the submitted ScheduleExpression>.`); the service SdkId, here `Scheduler`; the status code, here `400`; the request id and request token, which are per-run values and are NOT requirements; and the HandlerErrorCode, here `InvalidRequest`. An optional ` (SDK Attempt Count: <n>)` may follow the inner parenthesised clause.
Emulator: the stack does reach CREATE_FAILED and the HandlerErrorCode is right, but the reason is `Resource handler returned message: "Invalid Schedule Expression <expr>." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` — the handler message is missing its `Invalid request provided: ` prefix and the entire ` (Service: Scheduler, Status Code: 400, Request ID: <id>)` clause is absent from inside the quoted message.
Why: this reason string is the only machine-readable record of WHICH downstream service refused a stack resource and with what HTTP status. Tooling and operators that parse it (the check does, with the same regex AWS's format satisfies) cannot extract the service or status code at all, so a failed deployment cannot be attributed to Scheduler, and a client that matches the documented handler-message form sees an unrecognised failure reason.
Fix: the resource-handler error-wrapping path of the CloudFormation emulator — where a refused provisioning call is converted into the ResourceStatusReason written onto the resource's CREATE_FAILED stack event. It must prefix the downstream message with the handler's own `Invalid request provided: ` classification and append the `(Service: <sdkId>, Status Code: <status>, Request ID: <id>)` clause carrying the refusing service's SdkId and HTTP status. The Scheduler-side ValidationException message itself (`Invalid Schedule Expression <expr>.`) already matches AWS and needs no change.
Traces: cloudformation__scheduler.deploy-scheduler-schedule-malformed-rate-expression-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator: 33/171 checks passed; cloudformation__scheduler 1/2.

[mismatch] cloudformation__scheduler.deploy-scheduler-schedule-malformed-rate-expression-rejected
AssertionError: CREATE_FAILED reason is not a resource-handler message: 'Resource handler returned message: "Invalid Schedule Expression rate(five minutes)." (RequestToken: <token>, HandlerErrorCode: InvalidRequest)'

The check's regex for AWS's form is:
  ^Resource handler returned message: "(?P<message>.*) \(Service: (?P<service>[A-Za-z0-9]+), Status Code: (?P<status>\d+), Request ID: [^)]*\)(?: \(SDK Attempt Count: \d+\))?" \(RequestToken: [^,]*, HandlerErrorCode: (?P<handler>[A-Za-z]+)\)$
and it then asserts (message, service, status, handler) == ('Invalid request provided: Invalid Schedule Expression rate(five minutes).', 'Scheduler', '400', 'InvalidRequest').

Expected ResourceStatusReason (template form; request id/token are per-run values):
  Resource handler returned message: "Invalid request provided: Invalid Schedule Expression rate(five minutes). (Service: Scheduler, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)
Actual: the string quoted above — no `Invalid request provided: ` prefix, no `(Service: ..., Status Code: ..., Request ID: ...)` clause.

The positive control in the same check passes: the identical template with ScheduleExpression='rate(5 minutes)' reaches CREATE_COMPLETE and ListSchedules/GetSchedule report the schedule with the stack's QueueArn and RoleArn.

## Steps to Reproduce
1. Build a template with an AWS::SQS::Queue, an AWS::IAM::Role trusting scheduler.amazonaws.com with an inline sqs:SendMessage policy on the queue, and an AWS::Scheduler::Schedule whose ScheduleExpression is the malformed 'rate(five minutes)', FlexibleTimeWindow {Mode: OFF}, Target {Arn: queue arn, RoleArn: role arn}.
2. cloudformation.create_stack(StackName=s, TemplateBody=<that template>, Capabilities=['CAPABILITY_IAM'], DisableRollback=True).
3. Poll describe_stacks until the status is terminal: it is CREATE_FAILED (correct).
4. describe_stack_events(StackName=<stack id>) and take the single CREATE_FAILED row for LogicalResourceId 'Schedule'; read its ResourceStatusReason.
5. On AWS the reason matches `Resource handler returned message: "Invalid request provided: Invalid Schedule Expression rate(five minutes). (Service: Scheduler, Status Code: 400, Request ID: <id>)" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)`. On the emulator the inner message is bare (`"Invalid Schedule Expression rate(five minutes)."`) with no Service/Status Code/Request ID clause and no `Invalid request provided: ` prefix.
Check: cloudformation__scheduler.deploy-scheduler-schedule-malformed-rate-expression-rejected.
