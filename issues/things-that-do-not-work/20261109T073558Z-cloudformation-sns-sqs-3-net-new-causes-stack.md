Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored cloudformation__sns__sqs.obs-cloudformation-3-notification-arns-stream-update-rollback-delete-and-stop-when-cleared cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails
Issue-Key: cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored|_collect#21|AssertionError|raise AssertionError(f"events never delivered: {sorted(want_ids - set(got))!r}")
Issue-Key: cloudformation__sns__sqs.obs-cloudformation-3-notification-arns-stream-update-rollback-delete-and-stop-when-cleared|_assert_joined#33|AssertionError|assert got == want, (
Issue-Key: cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails|pull#11|Timeout|for msg in resp.get("Messages", []):
Issue-Commit: 6537d79e9f76a83fe65f7d2499927078c6ec62b1

# cloudformation__sns__sqs: 3 net-new causes — stack events of a change-set-executed update never reach the NotificationARNs topic, notifications serialize an unset PhysicalResourceId as the literal 'null', and SNS-backed custom resources never complete

> _Note: this issue was identified against the code base at commit `6537d79e9f76a83fe65f7d2499927078c6ec62b1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteChangeSet

## Description
Three independent root causes in the CloudFormation/SNS notification and custom-resource paths. A fixer must close all three before submitting.

=== Cause 1 of 3: stack events of an update executed through a change set are not published to the stack's NotificationARNs topic ===
AWS: recorded behavior only for the delivery itself (Smithy: CreateChangeSet.NotificationARNs is a list of SNS topic ARNs, length 0..5, and DescribeStacks echoes NotificationARNs). Every StackEvent produced while a stack has a topic associated is published to that topic as an SNS Notification whose Message is a newline-separated Key='value' body. For an UPDATE driven by CreateChangeSet+ExecuteChangeSet this includes the stack-level and resource-level events of that execution: the set of EventIds delivered to the topic equals the set of new EventIds DescribeStackEvents reports, minus only the clearing update's own terminal stack UPDATE_COMPLETE when that same update set NotificationARNs=[].
Emulator: the events are simply never delivered. After an ExecuteChangeSet-driven update completes (UPDATE_COMPLETE, and DescribeStacks correctly echoes NotificationARNs=[] for the cleared stack and [<topic>] for the twin that never cleared), draining the subscribed queue for up to 120s yields none of the update's EventIds. The replay reports: AssertionError: events never delivered: ['1317b292-…', '21e3e2d1-…', '5f539761-…', 'abfcc5a7-…'] (those ids are this run's EventIds; the requirement is that every new EventId arrives).
Why: NotificationARNs is the only push channel CloudFormation offers for deployment progress. A client that subscribes a queue or a Lambda to a stack's topic — the standard pattern for gating a pipeline on a deployment — receives nothing and waits forever, while DescribeStacks reports success. It cannot even distinguish "delivery is off" from "delivery is broken", because the stack's NotificationARNs field reads back correctly.
Fix: the stack-event publisher — it must fan every StackEvent out to the stack's currently associated NotificationARNs, on the ExecuteChangeSet-driven update path as well as the direct CreateStack/UpdateStack path, resolving the association as of the moment the event is emitted.
Traces: cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored

=== Cause 2 of 3: a notification's PhysicalResourceId for an unassigned resource is the literal string 'null' instead of empty ===
AWS: recorded behavior only. In the Key='value' notification body, a field whose underlying StackEvent value is unset is rendered as the empty string (PhysicalResourceId=''), and every joined field of a notification equals the same-EventId event's field as DescribeStackEvents reports it — StackId, StackName, LogicalResourceId, PhysicalResourceId, ResourceType, ResourceStatus, ResourceStatusReason. A JSON null is never a value in this format.
Emulator: for an UPDATE_IN_PROGRESS event whose PhysicalResourceId DescribeStackEvents reports as '', the notification carries PhysicalResourceId='null' — the serializer is writing a JSON/Rust None through a generic stringifier instead of rendering it as empty. The replay reports: AssertionError: U1: notification '<EventId>' (Notified UPDATE_IN_PROGRESS) field PhysicalResourceId: DescribeStackEvents says '', notification says 'null' (the EventId is this run's value; the diverging rendering is the requirement).
Why: a consumer that parses the notification body and keys off PhysicalResourceId gets the four-character string 'null' as a resource identifier. Code that tests the field for emptiness to decide "not yet created" takes the wrong branch, and the notification contradicts DescribeStackEvents for the very same EventId.
Fix: the stack-event notification serializer — render an absent optional field as an empty value in the Key='value' body (consistently with ResourceStatusReason), never as 'null'.
Traces: cloudformation__sns__sqs.obs-cloudformation-3-notification-arns-stream-update-rollback-delete-and-stop-when-cleared

=== Cause 3 of 3: a stack whose custom resources are backed by an SNS ServiceToken never reaches a terminal status ===
AWS: recorded behavior only (custom-resource protocol, https://docs.aws.amazon.com/AWSCloudFormation/latest/UserGuide/crpg-ref.html). CloudFormation publishes the custom-resource request (RequestType, ResponseURL, StackId, RequestId, LogicalResourceId, ResourceProperties) to the SNS ServiceToken, the provider PUTs a response document to the ResponseURL, and the stack proceeds. Where the response's Data lacks the key a consumer resolves with Fn::GetAtt, the consuming resource fails with the exact attribute error and the stack rolls back; where the key is present, the value is delivered into the consumer's request. Either way the stack settles in a terminal status.
Emulator: neither stack settles. The check exhausts its whole 180s budget polling DescribeStacks — reported as "Error: Check timed out after 180.0s" — so the custom-resource request is never published to the SNS ServiceToken (or its response is never consumed) and both the GetAtt-present and GetAtt-missing legs are unreachable.
Why: Custom:: resources backed by SNS are a supported provider type; a template using one never finishes deploying or failing, so a client blocks indefinitely on a stack that neither completes nor rolls back. The missing Fn::GetAtt attribute error is likewise never produced, so templates with a genuine authoring bug look like a hang rather than a validation failure.
Fix: the custom-resource provisioner's SNS ServiceToken dispatch and its response intake (the ResponseURL route), plus the Fn::GetAtt resolution of a custom resource's Data keys. Open issue 20261101T212352Z-cloudformation-3-net-new-behavioral-root-causes-waitconditionhandle.md records an SNS-backed-custom-resource cause under the `cloudformation` service and 20261108T043811Z-cloudformation-s3-a-lambda-backed-cloudformation-custom-resource-never-completes.md the Lambda-backed equivalent under `cloudformation__s3`; the suite bound this check to neither, so it is recorded here and should be closed alongside them.
Traces: cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails

## Evidence
cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored [mismatch]
  expected: every new EventId of the ExecuteChangeSet-driven update is delivered to the subscribed topic/queue within the timeout.
  actual:   AssertionError: events never delivered: ['1317b292-6993-1e17-e55f-ce33ccf6e05a', '21e3e2d1-ebc1-7bb9-daff-1e789e1de139', '5f539761-a55b-9500-b128-9d9cd8352af0', 'abfcc5a7-5e4b-12fb-faa3-f0c7714caea3']
  (the EventIds are per-run; the assertions that passed first — stacks reaching UPDATE_COMPLETE, DescribeStacks echoing NotificationARNs=[] for the cleared stack and [topic] for the twin — show the association is stored correctly and only the publish is missing.)

cloudformation__sns__sqs.obs-cloudformation-3-notification-arns-stream-update-rollback-delete-and-stop-when-cleared [mismatch]
  expected: notification field PhysicalResourceId == the same-EventId DescribeStackEvents value, i.e. '' when unassigned.
  actual:   AssertionError: U1: notification '038d67ec-4feb-8218-9355-cdc0b4542345' (Notified UPDATE_IN_PROGRESS) field PhysicalResourceId: DescribeStackEvents says '', notification says 'null'

cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails [mismatch]
  expected: both stacks (Custom::Source provider + Custom::Sink consumer over an SNS ServiceToken) settle — the Fn::GetAtt-present one in CREATE_COMPLETE with the value delivered, the Fn::GetAtt-missing one failed with the exact attribute error.
  actual:   Error: Check timed out after 180.0s — neither stack ever left *_IN_PROGRESS.

## Steps to Reproduce
Shared setup for causes 1 and 2: sns.CreateTopic; sqs.CreateQueue; SetQueueAttributes with a Policy allowing sns.amazonaws.com SendMessage from that topic; sns.Subscribe(Protocol='sqs', Endpoint=<queue arn>).

Cause 1 — cloudformation__sns__sqs.change-set-empty-notification-arns-stops-delivery-until-restored:
  CreateChangeSet(ChangeSetType='CREATE', NotificationARNs=[topic], TemplateBody with one AWS::CloudFormation::WaitConditionHandle) twice (a 'cleared' stack and a 'twin'); ExecuteChangeSet both; wait for CREATE_COMPLETE. Then CreateChangeSet(ChangeSetType='UPDATE', NotificationARNs=[]) on the first and CreateChangeSet(ChangeSetType='UPDATE') with no NotificationARNs on the twin; ExecuteChangeSet both; wait for UPDATE_COMPLETE. Diff DescribeStackEvents against what the queue delivers: none of the new EventIds arrive.

Cause 2 — cloudformation__sns__sqs.obs-cloudformation-3-notification-arns-stream-update-rollback-delete-and-stop-when-cleared:
  Create a stack with NotificationARNs=[topic] containing an AWS::SSM::Parameter, then UpdateStack setting that parameter's Value to '' so the handler rejects it. Parse the Key='value' lines of each delivered SNS Message and compare every field to the same-EventId DescribeStackEvents entry: PhysicalResourceId reads 'null' where the event reports ''.

Cause 3 — cloudformation__sns__sqs.custom-resource-getatt-missing-data-key-fails:
  CreateStack with Resources {Provider: {Type: 'Custom::Source', Properties: {ServiceToken: <topic arn>}}, Consumer: {Type: 'Custom::Sink', Properties: {ServiceToken: <topic arn>, Value: {'Fn::GetAtt': ['Provider', 'Missing']}}}} (and a twin using an attribute the provider does return); poll DescribeStacks. Neither stack ever leaves *_IN_PROGRESS and no custom-resource request is published to the topic.
