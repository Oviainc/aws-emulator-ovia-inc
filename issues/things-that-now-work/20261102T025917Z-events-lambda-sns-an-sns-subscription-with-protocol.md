Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission
Issue-Key: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission|run#17|Timeout|bodies = _collect_messages(sqs, fixed_queue_url, timeout_s=90, marker=marker_fixed)
Issue-Commit: 9010f89773c9fbf121fd2271b9167862206a1831

# events__lambda__sns: an SNS subscription with Protocol='lambda' is never delivered, so neither the withheld invoke (missing permission) nor the granted one can be observed

> _Note: this issue was identified against the code base at commit `9010f89773c9fbf121fd2271b9167862206a1831`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new root cause in the SNS -> Lambda subscription delivery slice.

Cause 1 of 1: SNS Publish performs no delivery for a subscription whose Protocol is `lambda`, so a deployed topic+function+subscription pipeline produces no effect at all — and the presence or absence of the function's lambda:InvokeFunction grant for sns.amazonaws.com makes no difference.

AWS: Smithy (services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json) documents Publish as "If you send a message to a topic, Amazon SNS delivers the message to each endpoint that is subscribed to the topic. The format of the message depends on the notification protocol for each subscribed endpoint. When a messageId is returned, the message is saved and Amazon SNS immediately delivers it to subscribers", and SubscribeInput$Protocol admits `lambda` ("delivery of JSON-encoded message to an AWS Lambda function"). Recorded behavior: Publish to a topic that has a confirmed `lambda` subscription invokes the endpoint function asynchronously, once per published message, with the SNS Records envelope — but ONLY when the function's resource policy carries a lambda:InvokeFunction statement whose Principal is sns.amazonaws.com (the statement CDK's high-level `LambdaSubscription` adds and a bare `CfnSubscription` does not). With that statement absent, SNS cannot invoke the function, the function never runs, and the publish still answers 200 with a MessageId. The two legs together are the requirement: no invocation without the grant, exactly one invocation per publish with it.

Emulator: no `lambda`-protocol delivery exists. services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs builds its fan-out through `QueueTarget::of`, which returns `None` for every subscription whose `protocol != "sqs"` — its own comment says "Every other protocol (`http`, `lambda`, `email`, `firehose`, …) needs its own delivery path, which is not implemented here. The message is still RECORDED against the subscription". So Publish records the message against the lambda subscription, answers 200 with a MessageId, and nothing is ever invoked. The check deploys the SAME CDK app twice — once as a bare CfnSubscription (no permission, broken leg) and once through LambdaSubscription (permission present, positive control) — and observes the function's effect through an SQS sink the handler writes to. Both legs see an empty sink: the broken-leg assertion ("function was invoked despite missing permission") is satisfied vacuously, and the positive control then polls the sink until the replay cuts the check off — the suite reports "Check timed out after 60.0s". A second consequence of the same cause: because no delivery is attempted, the emulator never evaluates the function's resource policy, so a correctly-withheld delivery is indistinguishable from the absence of any delivery machinery.

Why: topic-to-function fan-out is how most SNS consumers are written. A client deploys the stack, every control-plane call (CreateTopic, CreateFunction, Subscribe with Protocol='lambda', AddPermission) returns 2xx, CloudFormation reports CREATE_COMPLETE, and Publish returns a MessageId — yet the function never runs and nothing reports a dropped notification. Worse, a permission-boundary test passes for the wrong reason: the emulator agrees that the un-permissioned subscription does not invoke, so a client cannot tell a correctly-enforced deny from a universally dead delivery path.

Fix: the SNS subscription delivery path — `QueueTarget::of` / `queue_deliveries` / `deliver_to_queues` in services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs (shared by PublishBatch and by the replay path) needs a `lambda`-protocol target alongside the `sqs` one that issues an asynchronous Invoke of the endpoint function through the in-process callback with the SNS Records envelope, gated on the function's resource policy granting lambda:InvokeFunction to sns.amazonaws.com (with the SourceArn condition honoured), and parks a delivery it cannot make in the subscription's RedrivePolicy dead-letter target.

Related filed work (neither covers this check): 20261025T002357Z-events-lambda-sns-eventbridge-sns-targets-never-publish-and-an-sns.md carries the replay key of the POSITIVE-ONLY sibling check events__lambda__sns.cdk-deploy-sns-lambda-subscription-notification and describes cause 2 there, but not this check's key nor the missing-permission leg; 20261022T213144Z-lambda-invoke-refuses-every-real-invocation-with-notimplemented.md covers the Lambda Invoke API stub, not the SNS-side delivery that would call it.

Traces: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission.

## Evidence
Failing check: [mismatch] events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission — "Error: Check timed out after 60.0s".

Sequence replayed, all control-plane calls 2xx: CreateQueue(sink-broken) + GetQueueAttributes(QueueArn); cdk synth + CreateStack of the SNS/Lambda app with context include_permission=false (bare sns.CfnSubscription(topic_arn, endpoint=<fn arn>, protocol='lambda')), stack reaching CREATE_COMPLETE with outputs TopicArn/FunctionName; Publish(TopicArn=<topic>, Message=<marker-broken>) -> 200 with a MessageId; ReceiveMessage drain of the sink for 40s -> empty (assertion satisfied). Then the same app with include_permission=true (CDK LambdaSubscription, which adds the lambda:InvokeFunction permission for sns.amazonaws.com); Publish(Message=<marker-fixed>) -> 200 with a MessageId; ReceiveMessage drain of the second sink for up to 90s -> no message ever arrives and the per-check budget expires.

Expected: the second (positive-control) publish invokes the function, whose handler calls SendMessage into its sink, so a body containing the published marker appears in that queue inside the delivery budget; the first publish invokes nothing.
Actual: no invocation on either leg; the sink queues stay empty until the deadline.

Emulator source: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/publish.rs — `QueueTarget::of` returns `None` whenever `subscription.protocol != SQS_PROTOCOL` ("sqs"), with the in-code note that `lambda` "needs its own delivery path, which is not implemented here"; `queue_deliveries` therefore yields no delivery for the lambda subscription and `deliver_to_queues` has nothing to perform. No status or error code diverges — Publish answers its modelled 200 with a MessageId on both legs — the divergence is the absent effect.

## Steps to Reproduce
1. CreateQueue(sink); GetQueueAttributes(sink, ['QueueArn']).
2. Deploy a stack (CDK app `sns_lambda_cdk` with --context include_permission=false, or plain CFN) declaring: an SNS topic, a Lambda function whose handler does SendMessage into the sink, and a bare AWS::SNS::Subscription with Protocol='lambda', Endpoint=<function arn> and NO AWS::Lambda::Permission for sns.amazonaws.com. Wait for CREATE_COMPLETE.
3. Publish(TopicArn=<topic>, Message='marker-broken') -> 200 + MessageId. Drain the sink for ~40s: expect nothing (AWS also delivers nothing here, because the invoke permission is missing).
4. Deploy the SAME app with include_permission=true (CDK `topic.add_subscription(subs.LambdaSubscription(fn))`, which also emits the lambda:InvokeFunction permission for sns.amazonaws.com) against a second sink queue.
5. Publish(TopicArn=<topic2>, Message='marker-fixed') -> 200 + MessageId. Drain the second sink: expect a message whose body contains 'marker-fixed' (the SNS Records envelope the function forwarded).
6. Observe the second drain never sees a message: the emulator performs no delivery for a `lambda`-protocol subscription at all, so the positive control runs to its deadline and the check is cut off at 60s.

Re-replay check: events__lambda__sns.cdk-deploy-sns-lambda-subscription-missing-permission.
