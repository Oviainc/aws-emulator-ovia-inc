Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.eventbridge-sqs-target-without-queue-policy-no-delivery events__lambda__sqs.sqs-queue-policy-explicit-deny-blocks-eventbridge-delivery events__lambda__sqs.eventbridge-sqs-queue-policy-wrong-principal-blocks-delivery events__lambda__sqs.eventbridge-sqs-target-arn-must-match-queue-arn events__lambda__sqs.cli-sqs-missing-queue-policy-put-targets-succeeds-but-no-delivery events__lambda__sqs.cli-update-sqs-queue-policy-enables-eventbridge-delivery events__lambda__sqs.cfn-deploy-eventbridge-rule-missing-sqs-policy-no-delivery events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq
Issue-Commit: b1cd28f1ca277d6a032df605f83af5017e934043

# events/lambda/sqs: EventBridge SQS targets ignore the queue policy (including explicit Deny) — RESOLVED

This was filed as cause 1 of an eight-cause bundle. **Cause 1 is what this file records as
fixed. The bundle's other seven causes are still open and are filed as
`issues/things-that-do-not-work/20261029T182228Z-events-lambda-sqs-lambda-targets-and-sqs-esms-never-fire-seven-remaining-causes.md`**,
carrying their thirty-two `Issue-Key` lines, their evidence and a note per cause on what the next
fixer faces. Nothing here closes them: thirty of the bundle's forty checks still fail, and they
are the ones in that file.

## Operation
PutTargets

## What was wrong

EventBridge delivered a matched event to a rule's Amazon SQS target whatever that queue's own
resource policy said: with no policy at all, with an explicit `Deny` for
`events.amazonaws.com`, with a policy naming a different principal, and with a grant whose
`aws:SourceArn` condition named a different rule. AWS refuses every one of those — nothing is
enqueued — so the emulator both fabricated deliveries AWS blocks and ignored the explicit `Deny`
clients use as a hard kill switch and as the basis of a data boundary.

## The rule it now follows

"For Lambda functions, Amazon SNS topics and Amazon SQS queues, EventBridge relies on
resource-based policies" (`PutTargets`), and the call it makes on a queue target is
`sqs:SendMessage` as the service principal `events.amazonaws.com`. A service principal holds no
identity-based policies, so the queue's own `Policy` attribute is the only document in force and
it alone decides the delivery, in IAM's own order: an explicit `Deny` about the invocation
refuses it whatever is granted beside it, and otherwise the invocation needs an `Allow` about
that principal, that action and that queue — no document, or statements about something else, is
IAM's implicit deny. A statement conditioned on `aws:SourceArn` or `aws:SourceAccount` is
decided against the invoking rule's ARN and its account, so a grant scoped to one rule
authorizes that rule's deliveries alone, and the decision is taken against the document the
queue carries at the instant of the delivery.

The dead-letter write is the same `sqs:SendMessage` by the same principal, so the dead-letter
queue's own policy decides it the same way. A withheld delivery is a delivery that FAILED: it is
counted on the rule's `FailedInvocations` and dead-lettered with the condition and the sentence
Amazon SQS gives it — `NO_RESOURCE`, and the answer that service gives a caller it has granted
nothing, which is the same answer it gives one naming a queue that is not there.

## Where it lives

* `services/service-com-amazonaws-eventbridge-awsevents/src/state.rs` —
  `target_policy_admits` (the decision, through the shared
  `service_common::iam_authorization` evaluation) and `queue_access_refusal` (the reported
  condition and sentence).
* `services/service-com-amazonaws-eventbridge-awsevents/src/ops/put_events.rs` —
  `queue_delivery_refusal`, asked before the queue target's `SendMessage` and before the
  dead-letter write.
* `services/service-com-amazonaws-eventbridge-awsevents/tests/all/queue_target_policy_authorization.rs`
  — the suite for the rule: a table over what the queue's document states, the decision taken
  against the document as it stands at the delivery, the dead-letter path, and the rule's
  invocation metrics.

## The checks that now pass

* `events__lambda__sqs.eventbridge-sqs-target-without-queue-policy-no-delivery`
* `events__lambda__sqs.sqs-queue-policy-explicit-deny-blocks-eventbridge-delivery`
* `events__lambda__sqs.eventbridge-sqs-queue-policy-wrong-principal-blocks-delivery`
* `events__lambda__sqs.eventbridge-sqs-target-arn-must-match-queue-arn`
* `events__lambda__sqs.cli-sqs-missing-queue-policy-put-targets-succeeds-but-no-delivery`
* `events__lambda__sqs.cli-update-sqs-queue-policy-enables-eventbridge-delivery`
* `events__lambda__sqs.cfn-deploy-eventbridge-rule-missing-sqs-policy-no-delivery`
* `events__lambda__sqs.eventbridge-sqs-target-missing-queue-policy-dlq` (of the bundle's cause 2)

## The same rule, still owed by two other target kinds

A Lambda function target and an SNS topic target are governed the same way, and each has its own
open filing:
`20261029T182228Z-events-iam-lambda-sqs-an-eventbridge-rule-with-no-lambda-invokefunction-grant.md`
and
`20261029T182228Z-events-lambda-sns-eventbridge-publishes-to-sns-topics-with-no-topic-policy-and-an-sns.md`.
The decision is stated once in `state::target_policy_admits` and takes the attempt's action and
resource, so those invokers reuse it rather than restating the rule.
