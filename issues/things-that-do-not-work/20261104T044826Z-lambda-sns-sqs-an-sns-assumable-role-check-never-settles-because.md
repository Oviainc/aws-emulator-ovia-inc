Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.iam-role-trust-sns-principal
Issue-Key: lambda__sns__sqs.iam-role-trust-sns-principal|_latency#9|Timeout|time.sleep(poll_s)
Issue-Commit: b4ead8169e48e6047949699a772a41fd75a6bc4d

# An SNS-assumable role check never settles because the SNS delivery it depends on never happens

> _Note: this issue was identified against the code base at commit `b4ead8169e48e6047949699a772a41fd75a6bc4d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssumeRole

## Description
Cause 1 of 1: a role whose trust policy names the sns.amazonaws.com service principal must be assumable by SNS, and the check proves it by observing the delivery that role authorizes.
AWS: recorded behavior only — the recording creates the role with an sns.amazonaws.com trust statement, wires it as the topic's delivery/feedback role, publishes, and then observes the resulting delivery (the subscribed function's log marker / the queue message) that only an assumable role can produce; on AWS this settles inside the suite's 60s budget.
Emulator: the check is cut off with "Error: Check timed out after 60.0s" — the role and topic configuration are accepted but no delivery is ever made, so there is nothing to observe and the poll never terminates.
Why: a client cannot tell a correctly trusted service principal from an untrusted one, because neither produces a delivery; the trust policy's effect is entirely unobservable.
Fix: the SNS Publish delivery path (to the Lambda/SQS subscription) and the STS service-principal assume-role path it depends on.
Traces: lambda__sns__sqs.iam-role-trust-sns-principal.

## Evidence
Expected (AWS): the sns.amazonaws.com-trusted role is assumed and the publish is delivered to the subscription.
Actual (emulator): 'Error: Check timed out after 60.0s' — no delivery is ever made.

## Steps to Reproduce
1. CreateRole with a trust policy naming Service: sns.amazonaws.com.
2. CreateTopic, attach that role (delivery/feedback role), and Subscribe a Lambda or queue.
3. Publish and poll the subscriber for the delivery. Observe the scenario never settles within 60s.
