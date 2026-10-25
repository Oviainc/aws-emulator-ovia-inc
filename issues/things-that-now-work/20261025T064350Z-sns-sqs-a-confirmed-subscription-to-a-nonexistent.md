Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs.a-subscription-to-a-nonexistent-queue-is-accepted-and-counted
Issue-Key: sns__sqs|ListSubscriptions|NotImplemented
Issue-Commit: 30ecb1282bd7333a88bd61f51b7f1fc433035291

# sns__sqs: a confirmed subscription to a nonexistent queue cannot be counted — ListSubscriptions answers NotImplemented (1 cause)

> _Note: this issue was identified against the code base at commit `30ecb1282bd7333a88bd61f51b7f1fc433035291`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSubscriptions

## Description
Cause 1 of 1: ListSubscriptions is an unimplemented scaffold stub, so the accept-then-fail contract for a dangling SQS endpoint ARN cannot be observed.
AWS: Smithy: ListSubscriptions takes an optional NextToken and returns 200 with `Subscriptions` (SubscriptionsList) plus NextToken; it is @paginated (inputToken/outputToken NextToken, items Subscriptions); its only modelled errors are AuthorizationErrorException (403), InternalErrorException (500) and InvalidParameterException (InvalidParameter, 400). Recorded behavior: SNS does not resolve an SQS endpoint at Subscribe time, so subscribing an endpoint ARN that names no queue returns a full SubscriptionArn with PendingConfirmation false, and that confirmed subscription is then listed by ListSubscriptions and counted among the topic's subscribers.
Emulator: the call raises botocore.exceptions.ClientError (NotImplemented) — 'An error occurred (NotImplemented) when calling the ListSubscriptions operation: operation ListSubscriptions is scaffolded but not implemented in this emulator'. NotImplemented is not one of the operation's modelled errors.
Why: any client that enumerates subscriptions account-wide (inventory, drift detection, a subscriber count) gets an unmodelled error instead of a list, so no SDK error branch matches and the dangling-subscription case is invisible.
Fix: implement the ListSubscriptions operation handler in services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/list_subscriptions.rs, returning the account's subscriptions (including ones whose endpoint resolves to nothing) and refusing a token it did not mint with InvalidParameter.
Related open issues that name the same stub but do not carry this check's replay key: 20261025T025317Z-sns-listsubscriptions-stub.md and 20261025T022656Z-lambda-sns-sns-listsubscriptions-is-an-unimplemented-scaffold.md.
Traces: sns__sqs.a-subscription-to-a-nonexistent-queue-is-accepted-and-counted.

## Evidence
Expected: ListSubscriptions returns HTTP 200 with a Subscriptions list that includes the confirmed subscription whose SQS endpoint ARN names no existing queue (Subscribe itself returns a full SubscriptionArn, PendingConfirmation false).
Actual: botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListSubscriptions operation: operation ListSubscriptions is scaffolded but not implemented in this emulator
(verdict: error; the check aborts in context.py -> client.py _make_api_call before any assertion runs)

## Steps to Reproduce
1. Clone the repo and run the behavioral suite against the built emulator.
2. CreateTopic; Subscribe with Protocol='sqs' and an Endpoint ARN naming a queue that does not exist — observe it is accepted with a full SubscriptionArn.
3. Call ListSubscriptions. The emulator answers error code NotImplemented instead of 200 with the subscription list.
4. Re-replay sns__sqs.a-subscription-to-a-nonexistent-queue-is-accepted-and-counted to confirm the fix.
