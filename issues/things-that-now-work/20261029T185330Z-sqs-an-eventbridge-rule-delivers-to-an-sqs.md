Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.cfn-deploy-sqs-queue-policy-missing-negative glue__sqs.eventbridge-rule-missing-sqs-resource-policy-negative
Issue-Key: glue__sqs.cfn-deploy-sqs-queue-policy-missing-negative|_assert_no_message_arrives#12|AssertionError|raise AssertionError(
Issue-Key: glue__sqs.eventbridge-rule-missing-sqs-resource-policy-negative|run#11|AssertionError|assert len(blocked) == 0, (
Issue-Commit: d4c3381173b09dc1be1da315e64a87a78d438272

# sqs: an EventBridge rule delivers to an SQS queue whose resource policy does not grant events.amazonaws.com sqs:SendMessage, so an unpolicied queue still receives the event (1 cause)

> _Note: this issue was identified against the code base at commit `d4c3381173b09dc1be1da315e64a87a78d438272`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendMessage

## Description
1 cause, grounded in two failing glue__sqs checks.

Cause 1 of 1: a cross-service delivery into SQS is performed without evaluating the queue's resource policy.
AWS: recorded behavior (no model member expresses this): an SQS queue is only writable by a service principal that the queue's Policy attribute grants. An EventBridge rule whose target is a queue with NO policy statement allowing Principal {"Service":"events.amazonaws.com"} to sqs:SendMessage on that queue ARN delivers NOTHING: the rule matches, EventBridge's delivery attempt is refused by SQS's resource-policy evaluation (AccessDenied for the events.amazonaws.com principal), and ReceiveMessage on the queue returns no messages for as long as the policy is missing. Adding the statement, everything else unchanged, makes the identical event arrive.
Emulator: the event is delivered anyway. Both checks read the queue after the rule matched and found the event body present on a queue carrying no policy at all (one of them deployed via CloudFormation with the AWS::SQS::QueuePolicy resource deliberately omitted, the other created the queue with no Policy attribute).
Why: resource-policy authorization on the SQS side of a cross-service delivery is not enforced, so the emulator is overly permissive: a deployment that silently drops events in AWS appears to work, every negative test of queue policies passes vacuously, and an operator gets no signal that the queue policy is missing — the defect only shows up in the real account.
Fix: the authorization of a SendMessage performed on behalf of a service principal — the queue resource-policy evaluation in the sqs service crate's SendMessage path (and the internal delivery path EventBridge targets use must go through it rather than writing to the queue state directly), so a delivery whose principal is not granted sqs:SendMessage on the queue ARN is refused.
Traces: glue__sqs.cfn-deploy-sqs-queue-policy-missing-negative, glue__sqs.eventbridge-rule-missing-sqs-resource-policy-negative.

## Evidence
glue__sqs.cfn-deploy-sqs-queue-policy-missing-negative:
  expected: zero messages on the queue whose AWS::SQS::QueuePolicy was omitted from the stack
  actual:   AssertionError: expected no message on unpolicied queue, got 1: ['{"version":"0","id":"…","detail-type":"Glue Job State Change","source":"sim2real.glue.job","account":"000000000000","time":"…","region":"us-west-2","resources":[],"detail":{"jobName":"<job>","severity":"INFO","state":"SUCCEEDED","jobRunId":"<id>","message":"Job run succeeded"}}']
  (the ids, names and timestamps inside the body are per-run values; the requirement is that NO message is delivered)

glue__sqs.eventbridge-rule-missing-sqs-resource-policy-negative:
  expected: no delivery to a queue with no policy granting events.amazonaws.com sqs:SendMessage
  actual:   AssertionError: event was delivered to SQS despite the queue having no policy granting events.amazonaws.com sqs:SendMessage

## Steps to Reproduce
1. sqs:CreateQueue(QueueName=Q) and do NOT set a Policy attribute (confirm GetQueueAttributes returns no Policy).
2. events:PutRule(Name=R, EventPattern={'source':['my.test']}) and events:PutTargets(Rule=R, Targets=[{'Id':'1','Arn':<Q arn>}]).
3. events:PutEvents(Entries=[{'Source':'my.test','DetailType':'t','Detail':'{}'}]).
4. sqs:ReceiveMessage(QueueUrl=Q, WaitTimeSeconds=10) repeatedly for ~30s: there must be NO message, because SQS refuses the events.amazonaws.com principal.
5. sqs:SetQueueAttributes(Q, Policy=<statement allowing Principal Service events.amazonaws.com to sqs:SendMessage on Q's ARN>) and repeat step 3: the message must now arrive. The only difference between the two runs is the queue policy.
