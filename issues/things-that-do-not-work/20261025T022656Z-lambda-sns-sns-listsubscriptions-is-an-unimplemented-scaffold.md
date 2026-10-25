Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns.snstrig-cli-jmespath-query-selects-the-one-non-drain-lambda-subscription lambda__sns.snstrig-lambda-policy-side-derivation-agrees-with-subscription-side lambda__sns.snstrig-sns-trigger-is-not-an-event-source-mapping lambda__sns.snstrig-subscription-outlives-its-deleted-function lambda__sns.snstrig-unsubscribe-removes-the-trigger-but-not-the-invoke-permission
Issue-Key: lambda__sns.snstrig-cli-jmespath-query-selects-the-one-non-drain-lambda-subscription|run#6|AssertionError|assert proc.returncode == 0, f"aws sns list-subscriptions exited {proc.returncode}: {proc.stderr}"
Issue-Key: lambda__sns|ListSubscriptions|NotImplemented
Issue-Commit: f04105fb2faf5f6bd82b29a86b394ae1fc089f7b

# lambda__sns: SNS ListSubscriptions is an unimplemented scaffold stub, so no Lambda trigger can be discovered through SNS (1 cause)

> _Note: this issue was identified against the code base at commit `f04105fb2faf5f6bd82b29a86b394ae1fc089f7b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscriptions

## Description
Cause 1 of 1: sns:ListSubscriptions answers NotImplemented, so every check that must enumerate a topic's subscriptions to reason about a Lambda trigger fails, including the AWS CLI path (which exits 254).

AWS: Smithy (sns) ListSubscriptions answers 200 with Subscriptions (SubscriptionArn, Owner, Protocol, Endpoint, TopicArn) and an optional NextToken; its modeled refusals are InvalidParameterException, InternalErrorException and AuthorizationErrorException. It is the account-wide enumeration a client uses to find which functions a topic triggers, to confirm an unsubscribe took effect, and to see that a subscription outlives the function it points at.

Emulator: the call is refused with code 'NotImplemented' and message 'operation ListSubscriptions is scaffolded but not implemented in this emulator' (recorded message, an example); through the CLI the same refusal surfaces as a non-zero exit (254).

Why: NotImplemented is not in the operation's modeled error set. A client cannot discover SNS-based Lambda triggers at all — it cannot tell an SNS trigger from an event source mapping, verify that Unsubscribe removed the trigger (while leaving the invoke permission), or detect a dangling subscription to a deleted function. Its error handling for ListSubscriptions is never exercised.

Fix: implement the ListSubscriptions handler in the sns service (account-wide subscription enumeration with pagination), alongside the already-served ListSubscriptionsByTopic.

Traces: the five lambda__sns checks in trace_ids.

## Evidence
Expected: ListSubscriptions returns 200 with the subscriptions of the account (Protocol='lambda', Endpoint=<function arn>).
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListSubscriptions operation: operation ListSubscriptions is scaffolded but not implemented in this emulator — and via the CLI: 'aws sns list-subscriptions exited 254'.

## Steps to Reproduce
1. Create a topic and subscribe a Lambda function to it.
2. Call ListSubscriptions (SDK) and `aws sns list-subscriptions` (CLI).
3. Observe NotImplemented / exit 254.
