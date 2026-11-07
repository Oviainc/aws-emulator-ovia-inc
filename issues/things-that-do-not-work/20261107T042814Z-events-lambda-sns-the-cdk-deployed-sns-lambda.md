Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission
Issue-Key: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission|_collect_messages#19|Timeout|return found
Issue-Commit: 69fbb491696def14b5ee7dd45913958907d046a9

# events__lambda__sns: the CDK-deployed SNS->Lambda subscription never invokes the function, so the permission-boundary positive control never converges (1 cause)

> _Note: this issue was identified against the code base at commit `69fbb491696def14b5ee7dd45913958907d046a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1: a CDK-deployed stack whose SNS topic is subscribed to a Lambda function WITH the lambda:InvokeFunction permission for sns.amazonaws.com still never invokes the function, so the paired negative (the same stack with the permission omitted) is ungradable. Failing check: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission.

AWS: a Subscribe with Protocol `lambda` and an Endpoint naming a function makes SNS invoke that function asynchronously for every Publish on the topic, PROVIDED the function's resource policy grants lambda:InvokeFunction to the sns.amazonaws.com principal with the topic ARN as SourceArn; the handler receives the standard SNS notification envelope (Records[].Sns with Message, MessageId, TopicArn). Without that AddPermission grant, SNS cannot invoke the function and the notification is never delivered — the asymmetry between the two stacks is the whole requirement. recorded behavior only for the exact envelope body.

Emulator: neither stack produces an invocation. The positive control — the identical CDK app WITH the AddPermission grant in place — never invokes the function either, so the check exhausts the harness budget: `Check timed out after 60.0s`. Hypothesis: the timeout line carries no further diagnosis; an absence on both sides means the SNS lambda-protocol subscription delivery path produces no invoke at all, so the authorization asymmetry it is supposed to enforce is unobservable.

Why: a client cannot tell a correctly-blocked subscription (the missing grant) from a subscription plumbing that never delivers; a stack with a genuine permission bug and a correct one look identical, and the whole SNS fan-out-to-Lambda pattern is inert.

Fix: SNS notification delivery for Protocol `lambda` subscriptions — the invoke dispatch and its lambda resource-policy (sns.amazonaws.com / SourceArn) authorization check.

Related open issue 20261104T044826Z-events-lambda-sns-eventbridge-rules-and-sns-subscriptions-never-deliver-to.md describes SNS subscriptions not delivering to Lambda for other checks in this grouping and does NOT carry this check's replay key.

Traces: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission.

## Evidence
Expected (AWS): the CDK stack that includes the lambda:InvokeFunction grant for sns.amazonaws.com invokes the function on Publish (the handler's effect becomes observable), while the otherwise-identical stack omitting the grant never does.
Actual (emulator): `Error: Check timed out after 60.0s` — no invocation is observed for EITHER stack, so the positive control that makes the negative meaningful never converges.
Note on scope: this check gates on the `cdk` tool being present; the run reached its body and timed out rather than being skipped, so the tool was available and the absence is the emulator's.

## Steps to Reproduce
1. Deploy the CDK app with an SNS topic, a Lambda function, a Protocol=`lambda` subscription, and AddPermission granting lambda:InvokeFunction to sns.amazonaws.com with the topic as SourceArn.
2. Publish to the topic and poll for the handler's effect. Nothing is observed.
3. Deploy the same app with the AddPermission resource omitted and repeat — indistinguishable from step 2.
4. Re-replay the check in trace_ids to confirm.
