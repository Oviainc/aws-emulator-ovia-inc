Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.cfn-deploy-eventbridge-rule-lambda-target
Issue-Key: events__lambda__sqs.cfn-deploy-eventbridge-rule-lambda-target|_poll_until#22|Timeout|last = probe()
Issue-Commit: 3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713

# events__lambda__sqs: a CloudFormation-deployed EventBridge rule with a Lambda target never invokes the function, so a matching PutEvents produces nothing downstream

> _Note: this issue was identified against the code base at commit `3ae1c5f9ad044bf0dd64ac25eb19f35c30d1d713`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause. The failing check is not marked OWNED or ALREADY FILED in the replay report.

AWS: recorded behavior only for the delivery itself; the control plane is modeled (events PutRule/PutTargets/ListTargetsByRule, lambda AddPermission/GetPolicy). When a stack has created an AWS::Events::Rule with an event pattern, an AWS::Events::Rule Target naming a Lambda function ARN, and an AWS::Lambda::Permission granting events.amazonaws.com lambda:InvokeFunction with an AWS:SourceArn condition on that rule ARN, a PutEvents entry matching the pattern causes EventBridge to invoke the function asynchronously with the full event envelope (version, id, detail-type, source, account, time, region, resources, detail). The function then forwards it onward (here: SendMessage to SQS), so the envelope becomes observable within seconds.
Emulator: the control plane is consistent — the stack's Outputs resolve to real ARNs, ListTargetsByRule reports exactly the function ARN, and GetPolicy shows the events.amazonaws.com statement scoped to the rule ARN (the check asserts all of this and those assertions pass). But a matching PutEvents never invokes the target: the check polls SQS for 60 s for the correlated envelope and never sees it, so the check is abandoned ("Check timed out after 60.0s") and never reaches a verdict.
Why: EventBridge is wired but inert. A client that builds an event-driven pipeline declaratively observes a fully configured rule whose events are silently dropped, and no amount of control-plane inspection reveals it. Every behaviour layered on delivery (pattern matching, input transformers, fan-out, dead-lettering) is unobservable.
Fix: the EventBridge PutEvents dispatch path — matching an entry against enabled rules and delivering it to each target, including the Lambda target type (honouring the target's resource policy check).
Traces: events__lambda__sqs.cfn-deploy-eventbridge-rule-lambda-target.

## Evidence
Expected: after PutEvents with Source "custom.cfntest" and Detail {"probe": "cfn", "correlation_id": <id>}, the rule's Lambda target runs and the forwarded EventBridge envelope arrives on the stack's SQS queue, with source == "custom.cfntest" and detail.probe == "cfn".
Actual (replay): `Error: Check timed out after 60.0s` — the SQS poll (`receive_message`, MaxNumberOfMessages=10, WaitTimeSeconds=5, 60 s budget) never returns the correlated message, while the preceding control-plane assertions (ListTargetsByRule == [function ARN]; GetPolicy contains events.amazonaws.com with ArnLike AWS:SourceArn == the rule ARN) all pass. The correlation id is a per-run value.

## Steps to Reproduce
1. CreateStack with a template declaring a Lambda function that forwards its event to an SQS queue, an AWS::Events::Rule with an event pattern on source "custom.cfntest" targeting that function, an AWS::Lambda::Permission for events.amazonaws.com scoped to the rule ARN, and Outputs for the function ARN, rule ARN and queue URL.
2. Wait for CREATE_COMPLETE; confirm ListTargetsByRule and lambda GetPolicy are as declared.
3. Call events PutEvents with an entry matching the pattern.
4. Poll SQS ReceiveMessage on the queue — no message ever arrives; the function was never invoked.
