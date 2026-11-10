Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create
Issue-Key: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create|_poll#25|Timeout|time.sleep(interval_s)
Issue-Commit: f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8

# cloudformation__lambda__logs: an AWS::Logs::SubscriptionFilter whose Lambda has no logs.amazonaws.com permission neither fails the stack nor delivers — the check reaches no verdict (1 cause)

> _Note: this issue was identified against the code base at commit `f4970b2d8a83c111b3a18edc79ceb1ab4d4e7ac8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: the create of a subscription filter targeting a Lambda with no AWS::Lambda::Permission for logs.amazonaws.com never resolves either way.
AWS: CloudFormation's AWS::Logs::SubscriptionFilter create calls PutSubscriptionFilter, which refuses a Lambda destination the logs service may not invoke with 400 InvalidParameterException 'Could not execute the lambda function. Make sure you have given CloudWatch Logs permission to execute your function.' (fixed text; no variable part), the resource goes CREATE_FAILED with that message as its status reason and the stack rolls back; the same template with the permission reaches CREATE_COMPLETE and a matching log event is forwarded to the function.
Emulator: the check produces no verdict at all — 'Error: Check timed out after 60.0s'. No CREATE_FAILED/ROLLBACK_COMPLETE and no delivery is observed inside the budget.
Why: a customer gets neither the refusal that tells them the permission is missing nor a working delivery, so the whole permission-gated subscription-filter contract is unverifiable from a template.
Fix: the logs PutSubscriptionFilter handler's destination-permission validation and the cloudformation AWS::Logs::SubscriptionFilter handler that must surface its refusal as CREATE_FAILED.
Traces: cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create.

## Evidence
Detail: 'Error: Check timed out after 60.0s'. Expected: the no-permission stack ends ROLLBACK_COMPLETE with the filter's ResourceStatusReason carrying Logs' InvalidParameterException sentence, and the permissioned twin delivers the marker event to the function. Actual: no stack terminal status and no delivery within the cap.

## Steps to Reproduce
Replay cloudformation__lambda__logs.cfn-logs-subscription-filter-missing-permission-rejected-at-create: deploy a template with a log group, a function with no AWS::Lambda::Permission for logs.amazonaws.com and an AWS::Logs::SubscriptionFilter pointing at it; poll DescribeStacks/DescribeStackEvents for the create outcome.
