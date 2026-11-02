Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative
Issue-Key: events__lambda__s3__stepfunctions.cfn-deploy-lambda-role-missing-events-permission-negative|_poll_until#17|Timeout|last = probe()
Issue-Commit: 62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3

# events__lambda__s3__sfn: a CFN stack whose Lambda role lacks events:PutEvents never reaches a verdict, so the negative deploy check hangs to the harness budget

> _Note: this issue was identified against the code base at commit `62e4c5cd3531cb5ea16934ee4e8b2e43c8dd65a3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: the deploy-time negative control for a missing events:PutEvents permission never settles.

AWS: recorded behavior only (CloudFormation's model describes the stack lifecycle but not this authorization outcome). AWS behaviour exercised by the check: a stack that provisions a Lambda whose execution role omits events:PutEvents reaches CREATE_COMPLETE (IAM shape is valid), and the deployed function's PutEvents call is then refused with AccessDeniedException naming the role and the events:PutEvents action — the rule's downstream target therefore receives nothing, which is the negative control for the sibling positive stack.

Emulator: the check never produces that verdict — `Error: Check timed out after 60.0s`. Either the stack never reaches a terminal status or the function invocation that must surface the denial never answers, so the "denied, hence no delivery" conclusion is never reached.

Why: a negative control that hangs is indistinguishable from a pipeline that silently delivers. A client cannot establish that a missing events:PutEvents grant is what blocked delivery, which is exactly the signal IaC tests rely on when tightening least-privilege roles.

Fix: the CloudFormation create path for this stack (reaching a terminal stack status) and the Lambda invoke path that must return the function's AccessDeniedException from PutEvents instead of never answering. Related open issue 20261029T182228Z-events-lambda-s3-sfn-eventbridge-fan-out-never.md covers the positive fan-out side of this fixture group but does not carry this check.

Traces: events__lambda__s3__sfn.cfn-deploy-lambda-role-missing-events-permission-negative.

## Evidence
Replay verdict [mismatch] for events__lambda__s3__sfn.cfn-deploy-lambda-role-missing-events-permission-negative; detail: `Error: Check timed out after 60.0s`. Expected: stack CREATE_COMPLETE, then the function's PutEvents refused with AccessDeniedException (role not authorized to perform events:PutEvents) and no delivery to the rule's target. Actual: no terminal outcome within the 60s budget.

## Steps to Reproduce
1. CreateStack with a Lambda function plus an execution role that grants logs only (no events:PutEvents) and an EventBridge rule with a downstream target.
2. Wait for the stack to reach CREATE_COMPLETE.
3. Invoke the function so it calls PutEvents: AWS returns the function's AccessDeniedException for events:PutEvents and the target receives nothing.
4. Against the emulator the check never reaches either observation and is killed at the 60s harness budget.
