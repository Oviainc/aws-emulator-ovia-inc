Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission
Issue-Key: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission|_poll_until#14|Timeout|time.sleep(interval_s)
Issue-Commit: e6f9f921790ee819fe5eaf1c5770366ea7fcc84b

# An SNS subscription with Protocol=lambda never invokes the function, so a CDK-deployed topic→Lambda pipeline cannot prove its permission boundary (1 cause)

> _Note: this issue was identified against the code base at commit `e6f9f921790ee819fe5eaf1c5770366ea7fcc84b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Cause 1 of 1 — SNS delivery for the `lambda` protocol never invokes the subscribed function, so a negative/positive pair that distinguishes "no lambda:InvokeFunction permission for sns.amazonaws.com" from "permission present" can never reach its positive control. Failing check: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission.

AWS: Smithy (com-amazonaws-sns-amazonsimplenotificationservice) documents SubscribeInput.Protocol as including `lambda` — "delivery of JSON-encoded message to an Lambda function". With a subscription whose Protocol is `lambda` and Endpoint is a function ARN, every Publish to the topic invokes that function asynchronously with the SNS `Records` envelope (Records[0].Sns.Message carries the published Message), but ONLY when the function's resource policy grants lambda:InvokeFunction to the sns.amazonaws.com principal for that topic. The permission is the gate: a bare subscription created without it (CloudFormation AWS::SNS::Subscription / CDK CfnSubscription, as opposed to the high-level LambdaSubscription construct which also emits the AWS::Lambda::Permission) is accepted and listed, yet no invocation ever occurs; add the permission and the same topic/function pair delivers. Both legs of that contrast are observable: the function forwards each invocation event to an SQS queue, so the queue is empty for the un-permitted stack and carries the published marker for the permitted one. The model says nothing about the delivery side-effect itself, so the gating rule above is recorded behavior only.

Emulator: no invocation happens in either leg. Every control-plane call succeeds — the CDK-synthesized stack reaches CREATE_COMPLETE with its Outputs (TopicArn, FunctionName), CreateTopic / CreateFunction / the SNS subscription and the SQS grant all return 2xx, and sns:Publish returns 200 with a MessageId — but the function is never run, so nothing is ever written to the sink queue. The negative leg therefore "passes" vacuously (nothing arrives, but for the wrong reason), and the positive control, which polls the sink for the published marker, never observes it; the check is cut off by the replay at its 60 s budget with no message ever delivered.

Why: a client that wires topic→function fan-out — the most common SNS consumer shape — gets a stack that reports healthy, a subscription that lists as confirmed and a Publish that returns a MessageId, while no handler ever runs. Worse for test and policy work, the emulator cannot distinguish a correctly denied delivery from a broken one: because delivery never happens at all, a missing lambda:InvokeFunction grant produces the same observable as a correct grant, so a permission-boundary regression in the client's own templates is undetectable.

Fix: the SNS subscription delivery path for the `lambda` protocol in the SNS crate (the Publish fan-out that dispatches per subscription) and the asynchronous Lambda invocation it must issue, gated on the target function's resource policy granting lambda:InvokeFunction to sns.amazonaws.com (with the aws:SourceArn topic condition) — deliver when the statement is present, drop when it is not. Related open issues, neither of which carries this check's replay key: 20261025T002357Z-events-lambda-sns-eventbridge-sns-targets-never-publish-and-an-sns.md (its Cause 2 describes the same lambda-protocol delivery gap but is grounded only in the sibling positive check events__lambda__sns.cdk-deploy-sns-lambda-subscription-notification, not in this permission-boundary check), and 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md (the Invoke API stub, which is the sink of this path but not the SNS-side dispatch or the permission gate).

Traces: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission.

## Evidence
Check events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission — verdict [mismatch], detail "Check timed out after 60.0s".

The check deploys the same asset-free CDK app twice through plain CloudFormation, differing only in the synth context `include_permission`:
  - include_permission=false → sns.CfnSubscription(topic_arn, endpoint=<function arn>, protocol='lambda') with NO AWS::Lambda::Permission.
  - include_permission=true  → topic.add_subscription(subs.LambdaSubscription(fn)), which also emits the sns.amazonaws.com lambda:InvokeFunction permission.
The inline handler forwards every invocation event into an externally created SQS queue.

Expected (AWS): the un-permitted stack's Publish produces no invocation, so the sink queue holds no message containing the broken marker (assertion "function was invoked despite missing permission"); the permitted stack's Publish does invoke, so the sink queue holds a message containing the fixed marker within the 90 s poll (assertion "positive control: function was never invoked").

Actual (emulator): all control-plane calls return 2xx — both stacks reach CREATE_COMPLETE with Outputs TopicArn/FunctionName, the subscription is created, and sns:Publish returns 200 with a MessageId for both markers — but neither leg ever invokes the function, so the sink queues stay empty for both the broken and the fixed deployment. The positive control therefore cannot be satisfied and the replay cuts the check off at its 60 s budget inside _collect_messages' receive_message loop. No status or error code diverges here; the divergence is the absent side-effect, identical in the permitted and un-permitted legs.

## Steps to Reproduce
1. CreateQueue(sink) and read its QueueArn.
2. Create a Lambda function with inline code that does sqs.send_message(QueueUrl=$QUEUE_URL, MessageBody=json.dumps(event)), with QUEUE_URL set to the sink and an execution role granting sqs:SendMessage on it.
3. CreateTopic(t).
4. Negative leg: create the subscription only — Subscribe(TopicArn=t, Protocol='lambda', Endpoint=<function arn>) (or an AWS::SNS::Subscription / CDK CfnSubscription in a stack) and add NO lambda AddPermission for sns.amazonaws.com. Publish(TopicArn=t, Message='marker-broken-<n>').
   Expected: the function is not invoked and the sink queue never receives 'marker-broken-<n>'.
5. Positive leg: repeat with a second topic/function/sink, this time ALSO calling lambda AddPermission(FunctionName=fn, Action='lambda:InvokeFunction', Principal='sns.amazonaws.com', SourceArn=<topic arn>) (what CDK's LambdaSubscription emits). Publish(TopicArn=t2, Message='marker-fixed-<n>').
   Expected: the function is invoked with the SNS Records envelope and the sink queue receives a body containing 'marker-fixed-<n>'.
   Observed on the emulator: both Publish calls return 200 with a MessageId, and NEITHER sink queue ever receives a message — step 5 never delivers, so the permitted and un-permitted cases are indistinguishable.
