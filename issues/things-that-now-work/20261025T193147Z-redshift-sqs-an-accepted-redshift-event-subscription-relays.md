Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift__sqs.cli-redshift-event-subscription-sqs-poll
Issue-Key: redshift__sqs.cli-redshift-event-subscription-sqs-poll|_run_shell#20|Timeout|result = subprocess.run(
Issue-Commit: 0ec51d1d5fd31e64b0ca9d7b1795f66a6201ed59

# redshift__sqs: an accepted Redshift event subscription relays no event — CreateClusterParameterGroup publishes no REDSHIFT-EVENT-1003 to the subscribed SNS topic (1 cause)

> _Note: this issue was identified against the code base at commit `0ec51d1d5fd31e64b0ca9d7b1795f66a6201ed59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSubscription

## Description
Cause 1 of 1: the Redshift event-publication path does not exist. A subscription is wired (SNS topic, SQS queue subscribed to it with a policy allowing sns.amazonaws.com to sqs:SendMessage, then `aws redshift create-event-subscription --source-type cluster-parameter-group --event-categories configuration --severity INFO`), a qualifying action is performed (`aws redshift create-cluster-parameter-group --parameter-group-family redshift-1.0`), and nothing is ever relayed to the queue.

AWS: Smithy (com-amazonaws-redshift-redshiftserviceversion20121201) models CreateEventSubscription with required SubscriptionName and SnsTopicArn plus SourceType, SourceIds, EventCategories, Severity, Enabled and Tags, returning an EventSubscription; its modeled errors are SubscriptionAlreadyExistFault, SNSTopicArnNotFoundFault (404 `SNSTopicArnNotFound`), SNSInvalidTopicFault, SNSNoAuthorizationFault, SourceNotFoundFault, SubscriptionCategoryNotFoundFault, SubscriptionSeverityNotFoundFault, SubscriptionEventIdNotFoundFault, EventSubscriptionQuotaExceededFault, InvalidTagFault and TagLimitExceededFault. CreateClusterParameterGroup requires ParameterGroupName, ParameterGroupFamily and Description and returns a ClusterParameterGroup. The relay itself is AWS-documented behaviour rather than modeled: creating a cluster parameter group raises Redshift event REDSHIFT-EVENT-1003, source type `cluster-parameter-group`, category `configuration`, severity INFO, which a matching subscription publishes to its SNS topic as a JSON notification whose fields include "Event Source" = `cluster-parameter-group`, "Resource" = the parameter group name, and "Event Message" of the fixed form `Cluster parameter group '<name>' was created at <YYYY-MM-DD HH:MM> UTC.` — the quoted group name and the timestamp are taken from the request and the server clock, the rest of that sentence is fixed text.

Emulator: nothing is ever delivered. The check is cut at the suite's 60 s cap (`Error: Check timed out after 60.0s`) inside its CLI polling loop over `aws sqs receive-message --wait-time-seconds 20` (own budget 280 s); no message mentioning the parameter group ever arrives, so the REDSHIFT-EVENT-1003 assertions are never reached. Note the failure has moved downstream: the open issue 20261024T024248Z-redshift-sqs-redshift-event-notification-operations-answer-notimplemented-so-no.md recorded this same check failing fast with `AssertionError: Command failed (rc=254) ... (NotImplemented) when calling the CreateEventSubscription operation`, and that refusal no longer appears — the CLI calls now get far enough for the run to spend its whole window polling an empty queue. Hypothesis: subscriptions are now registered but no Redshift control-plane action raises an event, so nothing is matched against them and nothing is published to SNS; neither the model nor the replay states which half is missing.

Why: every notification-driven Redshift workflow stays silent while looking healthy. A client that creates a subscription, gets a success back and then waits for configuration events waits forever, with no error to classify and nothing on the queue to distinguish 'no event raised' from 'event raised but not routed'.

Fix: the Redshift event source — the control-plane handlers that must raise catalog events (CreateClusterParameterGroup in services/service-com-amazonaws-redshift-redshiftserviceversion20121201/src/ops/ raising REDSHIFT-EVENT-1003) — plus the subscription-matching and SNS publication path behind create_event_subscription.rs, which must filter by SourceType, EventCategories and Severity and publish the documented JSON notification to the subscription's topic.

Traces: redshift__sqs.cli-redshift-event-subscription-sqs-poll.

## Evidence
[mismatch] redshift__sqs.cli-redshift-event-subscription-sqs-poll
Error: Check timed out after 60.0s

Setup that succeeds: SQS queue + SNS topic + queue policy allowing sns.amazonaws.com sqs:SendMessage with Condition ArnEquals aws:SourceArn = <topic>, SNS subscribe(Protocol=sqs), `aws redshift create-event-subscription --subscription-name <n> --sns-topic-arn <topic> --source-type cluster-parameter-group --event-categories configuration --severity INFO`, `aws redshift create-cluster-parameter-group --parameter-group-name <g> --parameter-group-family redshift-1.0 --description ...`.

Expected: within the poll window one SQS message whose SNS envelope `Message` parses as JSON with "Event Source" = "cluster-parameter-group", "Resource" = <g>, and "Event Message" matching `Cluster parameter group '<g>' was created at <YYYY-MM-DD HH:MM> UTC.`
Observed: the queue stays empty for the whole run; no message, no error.

(The parameter group name, subscription name, topic ARN and the event's timestamp are per-run values filling that template; the fixed sentence and the three field names are the requirement.)

## Steps to Reproduce
1. Boot the emulator; point sqs, sns and redshift clients and the AWS CLI at it.
2. Create an SQS queue, an SNS topic, set a queue policy allowing sns.amazonaws.com to sqs:SendMessage on the queue for that topic ARN, and subscribe the queue to the topic (Protocol=sqs).
3. `aws redshift create-event-subscription --subscription-name <n> --sns-topic-arn <topic> --source-type cluster-parameter-group --event-categories configuration --severity INFO`. Expect an EventSubscription in the response.
4. `aws redshift create-cluster-parameter-group --parameter-group-name <g> --parameter-group-family redshift-1.0 --description sim2real`.
5. Long-poll the queue (`aws sqs receive-message --wait-time-seconds 20`) for a message whose SNS `Message` names <g>. Expect the REDSHIFT-EVENT-1003 notification described above; observe that nothing is ever delivered.
6. Re-replay redshift__sqs.cli-redshift-event-subscription-sqs-poll to confirm the fix.
