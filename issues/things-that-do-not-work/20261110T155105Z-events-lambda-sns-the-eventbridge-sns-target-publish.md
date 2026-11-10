Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published
Issue-Key: events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published|_wait_for_matching_message#15|Timeout|resp = sqs.receive_message(
Issue-Commit: 8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f

# events__lambda__sns: the EventBridge SNS-target publish path reaches no graded outcome — the topic resource policy is not consulted and the rule→SNS→SQS delivery never settles

> _Note: this issue was identified against the code base at commit `8b7ff9be7fbfdfa6a874338015bc2fd39ac1e37f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
One root cause, grounded in a single failing check.

NOTE on prior art: open issue 20261029T182228Z-events-lambda-sns-eventbridge-publishes-to-sns-topics-with-no-topic.md (Cause 1) describes this same path and even lists this check id in its Issue-Trace-Ids, but it no longer carries the check's replay key (its key is `run#16|AssertionError|assert messages == []`, while the check now ends as a harness timeout), so the suite does not bind the check to it. It is re-filed here; a fixer should close that issue's Cause 1 together with this one.

Cause 1 of 1: an EventBridge rule whose target is an SNS topic is not authorized against that topic's resource policy, and the rule-triggered publish never produces an observable outcome either way.
AWS: recorded behavior only (the SNS Smithy model types the refusal but not the rule). EventBridge calls SNS Publish on an SNS target as the service principal events.amazonaws.com. Publish is refused when the topic's resource policy contains no Allow statement for that principal — Smithy: Publish declares AuthorizationErrorException (HTTP 403, awsQueryError code `AuthorizationError`); the refusal is not visible to the PutEvents caller, it surfaces as no delivery to any subscriber plus a FailedInvocation/dead-letter on the rule. Once an Allow statement for Principal {"Service": "events.amazonaws.com"} with Action SNS:Publish on that topic ARN (optionally Condition ArnEquals aws:SourceArn = the rule ARN) is added with SetTopicAttributes(AttributeName=Policy), the identical rule and target must deliver the event envelope to the topic's subscribers. The three facts AWS must make distinguishable are: (a) a direct Publish by the topic owner is delivered, (b) the rule-triggered publish with no grant is delivered to nobody, (c) after the grant, the same rule-triggered publish is delivered.
Emulator: the check reaches none of its graded assertions and ends "Check timed out after 60.0s". Nothing in the emulator's responses separates (b) from (c): the denied case and the restored case are indistinguishable, so the permission boundary on EventBridge's SNS target is ungraded. Previously recorded against this same check (open issue 20261029T182228Z, Cause 1): the subscriber received the notification carrying the full EventBridge envelope as `Message` even though the topic policy granted events.amazonaws.com nothing — i.e. the resource policy is not consulted on the SNS-target publish at all.
Why: a client cannot rely on the topic policy as a control: either the emulator fabricates a publish AWS refuses (a least-privilege rollout that breaks fan-out in AWS passes locally), or the rule→SNS path is inert and granting the statement never unblocks delivery. In both shapes a real authorization regression is invisible.
Fix: resource-policy authorization in the EventBridge SNS-target invoker and in the SNS Publish path it calls, and the SNS-target delivery itself, so that a grantless publish is refused (and counted as a failed invocation) while a granted one is delivered promptly.
Traces: events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published

## Evidence
[mismatch] events__lambda__sns.eventbridge-sns-target-missing-topic-policy-not-published — "Error: Check timed out after 60.0s". Expected, in order: a direct Publish by the topic owner observed on the subscribed queue; then PutEvents matching a rule whose only target is the topic produces NO message on that queue (the check's assertion is `assert messages == []`, "message was published without a topic policy grant"); then, after SetTopicAttributes adds Sid AllowEventBridgePublish — Effect Allow, Principal {"Service": "events.amazonaws.com"}, Action "SNS:Publish", Resource <topic arn>, Condition ArnEquals aws:SourceArn <rule arn> — an identical PutEvents under a fresh marker IS observed ("granting the topic policy did not unblock delivery of the identical rule/target"). Actual: no graded verdict is reached within the check's budget; the emulator's recorded behavior on this path (per open issue 20261029T182228Z) is to deliver the notification with no grant in place. Topic/queue/rule ARNs and markers are per-run values.

## Steps to Reproduce
CreateTopic; CreateQueue and allow sns.amazonaws.com sqs:SendMessage on it for that topic; Subscribe the queue to the topic; Publish directly as the owner and observe the envelope on the queue (positive control). PutRule with EventPattern {"source": [<unique source>]} State=ENABLED; PutTargets with the topic ARN and NO topic-policy statement for events.amazonaws.com; PutEvents one matching entry and poll the queue — AWS delivers nothing. Then SetTopicAttributes(AttributeName=Policy) adding Allow events.amazonaws.com SNS:Publish on the topic (Condition ArnEquals aws:SourceArn = the rule ARN), PutEvents an identical entry with a fresh marker and poll — AWS delivers it.
