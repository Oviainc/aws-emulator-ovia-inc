Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudfront__events.eventbridge-cloudfront-event-missing-detail-type-not-matched
Issue-Key: cloudfront__events.eventbridge-cloudfront-event-missing-detail-type-not-matched|_warm_up_role#10|Timeout|resp = lam.invoke(FunctionName=fn_name, Payload=b'{"warmup": true}', InvocationType="RequestResponse")
Issue-Commit: 34c55a01d6a9fc011f87ca85d6fbf4d994ca748c

# cloudfront__events: a rule that matches on detail-type never delivers the matching event to its Lambda target, so the detail-type negative passes vacuously and its positive control never completes

> _Note: this issue was identified against the code base at commit `34c55a01d6a9fc011f87ca85d6fbf4d994ca748c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One net-new root cause, grounded in the only failing cloudfront__events check in this replay that the report marks neither OWNED nor ALREADY FILED.

Cause 1 of 1: a PutEvents entry whose DetailType matches an ENABLED rule's EventPattern is accepted (FailedEntryCount 0) and then never dispatched to the rule's Lambda target, so the rule's detail-type discrimination cannot be observed at all — the negative leg (a differing detail-type must deliver nothing) is satisfied only because NOTHING is ever delivered, and the positive leg (the identical event with the required detail-type must be delivered with its detail intact) never produces a message. Failing check: cloudfront__events.eventbridge-cloudfront-event-missing-detail-type-not-matched.

AWS: Smithy (services/service-com-amazonaws-eventbridge-awsevents): PutEvents input Entries (@required, @length 1..10), output FailedEntryCount (integer, default 0) + Entries; its only modeled error is InternalException (HTTP 500) — i.e. an entry reported with FailedEntryCount 0 is an accepted promise of asynchronous delivery, there is no client-side refusal channel. PutRule input Name (@required, @length 1..64, @pattern ^[\.\-_A-Za-z0-9]+$), EventPattern (@length 0..4096), State (RuleState enum), errors include InvalidEventPatternException (HTTP 400). PutTargets input Rule (@required) + Targets (@required, @length 1..100). On AWS, an accepted entry is matched against each ENABLED rule's EventPattern on the named bus: a pattern that pins "detail-type": ["AWS API Call via CloudTrail"] matches ONLY an entry whose DetailType is exactly that string, and every other pattern member (source, detail.eventName, detail.requestParameters.distributionId) must also match. A matching entry invokes each target once — for a Lambda target, provided its resource policy admits events.amazonaws.com for that rule ARN (lambda:AddPermission Action lambda:InvokeFunction, Principal events.amazonaws.com, SourceArn arn:aws:events:<region>:<account>:rule/<ruleName>) — and the target receives the full event envelope, whose "detail-type" field is the published DetailType and whose "detail" is the published detail unchanged. A non-matching entry invokes nothing. Recorded behavior only for timing: the recording shows the delivery arriving well inside the check's poll window (the check allows 90 s).

Emulator: every setup call is accepted — sqs.CreateQueue + GetQueueAttributes(QueueArn); iam.CreateRole/AttachRolePolicy/PutRolePolicy for the relay function; lambda.CreateFunction plus a direct RequestResponse warm-up Invoke that succeeds (so function and role work); events.PutRule(State=ENABLED) with the detail-type-pinned pattern; lambda.AddPermission for events.amazonaws.com on the rule ARN; events.PutTargets with the single Lambda target; events.PutEvents for the mismatching detail-type (FailedEntryCount 0) and then for the matching one (FailedEntryCount 0). After that nothing happens: the mismatching entry correctly yields no message (vacuously), and the matching entry yields no message either. The check clears its 30 s negative window and is then killed in its 90 s positive-control poll by the harness: "Error: Check timed out after 60.0s". Nothing is refused — there is no status, error code or message to compare; the divergence is silent non-delivery.

Why: a client relying on EventBridge's event-pattern discrimination gets no signal of any kind. PutEvents reports success, so the publisher believes the event was handed off, and every downstream effect the target would have produced (here an SQS relay of the envelope; in real automation a CreateInvalidation, a write, a notification) is simply missing. Worse, the safety property the rule exists to provide is unverifiable: because non-delivery is universal, a pattern that is too broad is indistinguishable from one that is correctly narrow, so a mis-scoped rule looks correct in test and over-delivers (or under-delivers) in production. No DLQ record and no FailedInvocations signal distinguishes "did not match" from "matched but not delivered".

Fix: the EventBridge PutEvents -> rule-matching -> target-dispatch component for rules on the default bus: after an entry is accepted it must be evaluated against each ENABLED rule's EventPattern (including the detail-type member and nested detail members), and each matching target must be invoked under its permission check with the full event envelope (source, detail-type, detail preserved verbatim). The Lambda invoke leg this dispatcher drives is in scope with it, since the dispatch cannot be observed without it.

Related open issues (named for de-duplication; none of them carries this check's replay key, so this is filed separately and a single dispatch fix may well close them together): 20261102T091831Z-cloudfront-events-a-matched-rule-never-invokes-its.md, 20261105T190602Z-cloudfront-events-2-net-new-causes-a-matched.md, 20261104T051153Z-cloudfront-events-a-matching-putevents-entry-never-invokes.md, 20261024T182555Z-cloudfront-events-a-matched-eventbridge-rule-never-produces.md, 20261029T175823Z-cloudfront-events-13-more-cloudfront-eventbridge-checks-fail.md, 20261101T214154Z-cloudfront-events-two-more-root-causes-a-matched.md.

Traces: cloudfront__events.eventbridge-cloudfront-event-missing-detail-type-not-matched (group overly-permissive, axis negative-delivery).

## Evidence
Replay of suite simparity-behavioral-aws-v1: 302/584 passed (51.7%); service cloudfront__events 11/55 (20%), 44 failed, 43 of which the report marks OWNED by an open issue or ALREADY FILED (stub-blocked) and were skipped.

[mismatch] cloudfront__events.eventbridge-cloudfront-event-missing-detail-type-not-matched — "Error: Check timed out after 60.0s".

Setup, all accepted by the emulator: CreateQueue(observation) + GetQueueAttributes(QueueArn); an IAM role trusted by lambda.amazonaws.com with an inline sqs:SendMessage grant on that queue; CreateFunction whose handler does sqs.send_message(MessageBody=json.dumps(event)); a direct warm-up Invoke that succeeds; PutRule(Name=<generated>, State=ENABLED, EventPattern={"source":["sim2real.cloudfront"],"detail-type":["AWS API Call via CloudTrail"],"detail":{"eventName":["CreateInvalidation"],"requestParameters":{"distributionId":["E<13 hex upper>"]}}}); lambda.AddPermission(Principal events.amazonaws.com, SourceArn the rule ARN); PutTargets(Rule, Targets=[{Id:"target-1", Arn:<fn arn>}]).

Leg 1 (negative): PutEvents one entry {Source:"sim2real.cloudfront", DetailType:"CloudFront Distribution State-Change", Detail:{eventName:"CreateInvalidation", requestParameters:{distributionId:<id>, invalidationBatch:{callerReference:"sim2real"}}}}. Expected (AWS): FailedEntryCount 0 and NO message on the observation queue within 30 s, because the detail-type differs from the pinned value. Actual (emulator): FailedEntryCount 0 and no message — this leg "passes", but vacuously (see leg 2).

Leg 2 (positive control, the identical event with the matching detail-type): PutEvents one entry with DetailType "AWS API Call via CloudTrail" and the same Detail. Expected (AWS): FailedEntryCount 0, then within the check's 90 s window one SQS message whose JSON body satisfies body["detail-type"] == "AWS API Call via CloudTrail" and body["detail"]["requestParameters"]["distributionId"] == the published distribution id (the fixed text here is the member names "detail-type"/"detail"/"requestParameters"/"distributionId" and the literal detail-type string "AWS API Call via CloudTrail"; the distribution id, queue/role/function/rule names and account are per-run values from the request, not part of the requirement). Actual (emulator): FailedEntryCount 0 and the observation queue never receives anything; the poll is still running when the harness kills the check at its 60 s cap, so neither assertion is reached.

No refusal is involved: there is no diverging HTTP status, error code or error message — both PutEvents calls return 200 with FailedEntryCount 0 and the divergence is the absent delivery. Not environment-dependent: generated names, the account id, the distribution id, host and port differ per run; the requirement is that an entry matching an ENABLED rule's EventPattern reaches the rule's permitted Lambda target with source/detail-type/detail preserved, and that one whose detail-type differs does not.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay suite simparity-behavioral-aws-v1, service cloudfront__events, check eventbridge-cloudfront-event-missing-detail-type-not-matched. Or by hand:
3. sqs.CreateQueue(QueueName=<obs>) and GetQueueAttributes(AttributeNames=["QueueArn"]).
4. iam.CreateRole with a trust policy for lambda.amazonaws.com, AttachRolePolicy(AWSLambdaBasicExecutionRole) and PutRolePolicy granting sqs:SendMessage on that queue ARN.
5. lambda.CreateFunction (python handler: `_sqs.send_message(QueueUrl=<obs url>, MessageBody=json.dumps(event)); return event`); wait Active and confirm a direct RequestResponse Invoke succeeds.
6. events.PutRule(Name=<rule>, State="ENABLED", EventPattern=json.dumps({"source":["sim2real.cloudfront"],"detail-type":["AWS API Call via CloudTrail"],"detail":{"eventName":["CreateInvalidation"],"requestParameters":{"distributionId":[<dist_id>]}}})).
7. lambda.AddPermission(FunctionName=<fn>, Action="lambda:InvokeFunction", Principal="events.amazonaws.com", SourceArn=arn:aws:events:<region>:<account>:rule/<rule>).
8. events.PutTargets(Rule=<rule>, Targets=[{"Id":"target-1","Arn":<fn arn>}]).
9. events.PutEvents with one entry Source="sim2real.cloudfront", DetailType="CloudFront Distribution State-Change", Detail=the CloudTrail-shaped detail. Assert FailedEntryCount == 0, then poll ReceiveMessage for 30 s and assert nothing arrives.
   Expected and actual both: no message.
10. events.PutEvents the identical entry but with DetailType="AWS API Call via CloudTrail". Assert FailedEntryCount == 0, then poll ReceiveMessage for up to 90 s.
    Expected (AWS): one message whose body has detail-type == "AWS API Call via CloudTrail" and detail.requestParameters.distributionId == <dist_id>.
    Actual (emulator): no message ever arrives; the check is killed at the harness's 60 s cap, so step 9's "no delivery" result carries no information.
