Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__iam__sns__sqs__sts.put-targets-target-arn-condition-restricts-to-sqs
Issue-Key: events__iam__sns__sqs__sts.put-targets-target-arn-condition-restricts-to-sqs|_error#23|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 37e3f599a06cc415cb3d05dd7e4df6099b0b715c

# EventBridge PutTargets is not gated by the events:TargetArn request condition: a target outside the allowed service is accepted

> _Note: this issue was identified against the code base at commit `37e3f599a06cc415cb3d05dd7e4df6099b0b715c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 1: PutTargets' authorization does not publish the request's target-ARN condition key, so a policy that restricts which targets a principal may attach does not constrain it.

AWS: PutTargets is authorized with the events:TargetArn request condition key in scope. A principal whose policy allows events:PutTargets only when events:TargetArn matches the SQS queue pattern is refused when it attaches an SNS topic target: the call fails with `AccessDeniedException` carrying AWS's standard no-matching-policy wording — `User: <principal arn> is not authorized to perform: events:PutTargets on resource: <rule arn> because no identity-based policy allows the events:PutTargets action` — with principal, action and resource taken from the request. Smithy: eventbridge#PutTargets takes Rule, EventBusName and Targets[].Arn and returns FailedEntryCount/FailedEntries; the condition-key set is a service fact — recorded behavior only, and the recording shows the out-of-scope target refused.

Emulator: the call succeeds, returning `{'FailedEntryCount': 0, 'FailedEntries': []}` with HTTP 200.

Why: a client that restricts a delivery principal to one target service (a common blast-radius control, e.g. 'this role may only wire rules to SQS') has no control at all; the rule can be pointed at any target. Because the allowed leg also succeeds, an enforced policy is indistinguishable from an inert one.

Fix: the PutTargets authorization step must publish events:TargetArn (per target entry) into the authorization context so conditioned statements are evaluated, and refuse the request when no statement matches.

Traces: events__iam__sns__sqs__sts.put-targets-target-arn-condition-restricts-to-sqs.

## Evidence
Expected: PutTargets with an SNS topic target is refused under a policy conditioned on events:TargetArn matching the SQS queue.
Actual: AssertionError: expected the call to be refused, but it succeeded: {'FailedEntryCount': 0, 'FailedEntries': [], …HTTPStatusCode: 200}

## Steps to Reproduce
1. Create an event bus, a rule, an SQS queue and an SNS topic.
2. Create a role whose policy allows events:PutTargets only when events:TargetArn equals the queue ARN; AssumeRole.
3. Call PutTargets with Targets[0].Arn = the SNS topic ARN; observe success.
