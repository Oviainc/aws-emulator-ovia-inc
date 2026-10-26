Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cli-redshift-event-subscription-sqs-poll redshift__sqs.sdk-redshift-describe-events-to-sqs-manual-forward
Issue-Key: redshift__sqs.cli-redshift-event-subscription-sqs-poll|_run_shell#15|AssertionError|raise AssertionError(
Issue-Key: redshift__sqs|DescribeEventCategories|NotImplemented
Issue-Commit: 768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5

# Redshift event-notification operations answer NotImplemented, so no event subscription can be created and no event categories can be read (2 root causes)

> _Note: this issue was identified against the code base at commit `768bbe4b7ea7973f1ee2efc8bf0f22146be54ed5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSubscription

## Description
Two root causes in the redshift event-notification surface, both reached from redshift__sqs end-to-end checks (one through the AWS CLI, one through the SDK).

Cause 1 of 2: CreateEventSubscription is a scaffold stub, so the SNS->SQS event relay cannot be wired at all.
AWS: CreateEventSubscription registers a subscription and returns an EventSubscription structure. Smithy (com-amazonaws-redshift-redshiftserviceversion20121201): CreateEventSubscription input requires SubscriptionName and SnsTopicArn, accepts SourceType, SourceIds, EventCategories, Severity, Enabled and Tags, and outputs EventSubscription; its modeled errors are SubscriptionAlreadyExistFault, SNSTopicArnNotFoundFault (404, `SNSTopicArnNotFound`), SNSInvalidTopicFault, SNSNoAuthorizationFault, SourceNotFoundFault, SubscriptionCategoryNotFoundFault, SubscriptionSeverityNotFoundFault, SubscriptionEventIdNotFoundFault, EventSubscriptionQuotaExceededFault, InvalidTagFault, TagLimitExceededFault — none of which is a generic not-implemented code.
Emulator: `aws redshift create-event-subscription --subscription-name <n> --sns-topic-arn <arn> --source-type cluster-parameter-group --event-categories configuration --severity INFO --tags ...` exits rc=254 with error code `NotImplemented` and message "operation CreateEventSubscription is scaffolded but not implemented in this emulator".
Why: no Redshift event can be relayed to SNS and on to SQS, so every notification-driven workflow is dead, and the failure code is one no SDK error model knows — clients cannot even classify it.
Fix: the CreateEventSubscription handler in services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/create_event_subscription.rs plus the subscription registry and the event-publication path into SNS. An open scaffold-stub issue was filed for `CreateEventSubscription` off redshift.event-subscription-ops-semantics; it does not carry this CLI/SQS replay key, which is why it is recorded here.
Traces: redshift__sqs.cli-redshift-event-subscription-sqs-poll.

Cause 2 of 2: DescribeEventCategories is a scaffold stub.
AWS: DescribeEventCategories is a read-only catalog call that always succeeds — Smithy shows input {SourceType} (optional) and output {EventCategoriesMapList}, with an EMPTY errors list, i.e. the operation models no failure mode at all.
Emulator: the call fails with error code `NotImplemented`, message "operation DescribeEventCategories is scaffolded but not implemented in this emulator".
Why: an operation AWS models as unfailing now fails. Clients that enumerate the valid event categories/severities before building a subscription (and the forwarding flow that reads them before polling SQS) abort on the first call, and any code that assumes this read cannot fail has no error path.
Fix: the DescribeEventCategories handler in services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/ — return the EventCategoriesMapList catalog, filtered by SourceType when supplied.
Traces: redshift__sqs.sdk-redshift-describe-events-to-sqs-manual-forward.

## Evidence
Cause 1 — redshift__sqs.cli-redshift-event-subscription-sqs-poll:
  AssertionError: Command failed (rc=254):
  cmd: ['aws', '--output', 'json', 'redshift', 'create-event-subscription', '--subscription-name', 'sim2real-behav-<id>', '--sns-topic-arn', 'arn:aws:sns:us-west-2:000000000000:sim2real-behav-<id>', '--source-type', 'cluster-parameter-group', '--event-categories', 'configuration', '--severity', 'INFO', '--tags', 'Key=sim2real-check,Value=cli-redshift-event-subscription-sqs-poll']
  stderr:
  aws: [ERROR]: An error occurred (NotImplemented) when calling the CreateEventSubscription operation: operation CreateEventSubscription is scaffolded but not implemented in this emulator
Expected: the subscription is created and returns an EventSubscription; the subsequent qualifying action (creating a cluster parameter group, REDSHIFT-EVENT-1003 under the `configuration` category) relays through SNS to the subscribed SQS queue.

Cause 2 — redshift__sqs.sdk-redshift-describe-events-to-sqs-manual-forward:
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeEventCategories operation: operation DescribeEventCategories is scaffolded but not implemented in this emulator
Expected: a successful EventCategoriesMessage with EventCategoriesMapList (the operation models no errors).

(subscription names, topic ARNs and generated ids are per-run examples.)

## Steps to Reproduce
1. Boot the emulator; point redshift, sns and sqs clients (and the AWS CLI) at it.
2. Cause 1: create an SNS topic and an SQS queue subscribed to it, then run `aws redshift create-event-subscription --subscription-name <n> --sns-topic-arn <topic> --source-type cluster-parameter-group --event-categories configuration --severity INFO`. Expect an EventSubscription in the response; observe rc=254 with error code NotImplemented. Then create a cluster parameter group and poll the queue for the relayed event.
3. Cause 2: call redshift DescribeEventCategories (with and without SourceType). Expect EventCategoriesMapList; observe error code NotImplemented.
4. Replay the trace_ids checks to confirm.
