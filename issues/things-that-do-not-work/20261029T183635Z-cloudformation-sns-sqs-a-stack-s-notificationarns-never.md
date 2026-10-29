Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.cfn-notification-arns-publish-every-stack-event cloudformation__sns__sqs.obs-cloudformation-3-notification-arns-stream-update-rollback-delete-and-stop-when-cleared cloudformation__sns__sqs.xsvc-cloudformation-3-notification-arns-cleared-stops-readded-resumes-delete-notifies cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason
Issue-Key: cloudformation__sns__sqs.cfn-notification-arns-publish-every-stack-event|_receive#25|Timeout|resp = sqs.receive_message(
Issue-Key: cloudformation__sns__sqs.obs-cloudformation-3-notification-arns-stream-update-rollback-delete-and-stop-when-cleared|_receive#24|Timeout|resp = sqs.receive_message(
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-notification-arns-cleared-stops-readded-resumes-delete-notifies|_receive#16|Timeout|resp = sqs.receive_message(
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason|pull#17|Timeout|resp = self.sqs.receive_message(QueueUrl=self.queue_url, MaxNumberOfMessages=10, WaitTimeSeconds=5)
Issue-Commit: dfc2bec1feea4e1eda9981ef982ae4c91760421c

# cloudformation__sns__sqs: a stack's NotificationARNs never receive any stack event, and an SNS-backed custom resource's FAILED response does not roll the stack back (2 causes)

> _Note: this issue was identified against the code base at commit `dfc2bec1feea4e1eda9981ef982ae4c91760421c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Two net-new root causes in the CloudFormation -> SNS notification path. Both were reached through the emulator's own CloudFormation and SNS endpoints; neither is covered by an open issue (the closest, 20261025T180059Z-cloudformation-custom-resource-providers-and-hook-configuration.md, owns the custom-resource *provider/hook configuration* checks, not the FAILED-response rollback below, and 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md owns DescribeStackEvents shape, not SNS publication).

Cause 1 of 2: NotificationARNs publish no stack-event notifications at all.
AWS: CreateStack/UpdateStack/DeleteStack take NotificationARNs (Smithy: CreateStackInput.NotificationARNs -> NotificationARNs, list of NotificationARN, max 5). AWS publishes one SNS notification for EVERY stack event of that stack - CREATE_IN_PROGRESS/CREATE_COMPLETE per resource and per stack, and the same for update, update-rollback and delete - to each listed topic, as a body of Key='value' lines (StackId, Timestamp, EventId, LogicalResourceId, Namespace, PhysicalResourceId, PrincipalId, ResourceProperties, ResourceStatus, ResourceStatusReason, ResourceType, StackName, ClientRequestToken). Clearing NotificationARNs with an explicit empty list stops the stream; omitting the parameter on an update leaves the current setting untouched. Recorded behavior only for the exact line set (the model carries no payload schema).
Emulator: the subscribed SQS queue receives nothing for the entire life of the stack, so every check that waits for the first notification runs to the harness cap and is cut off at 60 s. No notification is published for create, update, update-rollback or delete, and consequently the "cleared stops / re-added resumes" transition cannot be observed either.
Why: a client that wires stack progress to SNS (deployment gates, ChatOps, rollback alerting) sees total silence and concludes the stack never changed state. Nothing in the API response reveals the omission, so the failure mode is a hang, not an error.
Fix: the CreateStack/UpdateStack/DeleteStack handlers' stack-event emission path - every recorded stack event must be fanned out to the stack's current NotificationARNs - plus the stack-state store that must keep NotificationARNs across an update that omits the parameter and clear it on an explicit empty list.
Traces: cloudformation__sns__sqs.cfn-notification-arns-publish-every-stack-event, cloudformation__sns__sqs.obs-cloudformation-3-notification-arns-stream-update-rollback-delete-and-stop-when-cleared, cloudformation__sns__sqs.xsvc-cloudformation-3-notification-arns-cleared-stops-readded-resumes-delete-notifies (all three: "Check timed out after 60.0s").

Cause 2 of 2: a custom resource that answers FAILED does not fail the stack or carry its Reason.
AWS: for an AWS::CloudFormation::CustomResource backed by an SNS ServiceToken, CloudFormation publishes a Create request carrying ResponseURL/StackId/RequestId/LogicalResourceId. When the provider PUTs {"Status":"FAILED","Reason":"..."} to the ResponseURL, the resource goes CREATE_FAILED with that provider-supplied Reason as its ResourceStatusReason and the stack rolls back to ROLLBACK_COMPLETE. Recorded behavior only for the envelope field names; the state machine is AWS-documented.
Emulator: the check never gets that far - it runs to the 60 s cap, because the custom-resource request is never published to the SNS ServiceToken topic, so there is no ResponseURL to answer and the stack neither fails nor rolls back.
Why: a client using a custom resource as a deployment gate gets a stack that appears to succeed (or hangs) when its provider explicitly reported failure, and loses the provider's Reason, which is the only diagnosis the provider can return.
Fix: the custom-resource provisioning path of the stack-create handler (publish the request envelope to the SNS ServiceToken, serve the ResponseURL, and apply FAILED -> CREATE_FAILED + rollback with the supplied Reason).
Traces: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-failed-response-rolls-back-with-its-reason ("Check timed out after 60.0s").

## Evidence
Cause 1 - expected: at least one SNS notification per stack event delivered to the topic named in NotificationARNs; actual: the subscribed queue stays empty and all three checks are terminated by the harness with "Error: Check timed out after 60.0s". Cause 2 - expected: CREATE_FAILED with the provider's Reason and the stack at ROLLBACK_COMPLETE; actual: no custom-resource request is ever published to the SNS ServiceToken, and the check is terminated with "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. CreateTopic, create an SQS queue and Subscribe it to the topic with RawMessageDelivery=false.
2. CreateStack with NotificationARNs=[topicArn] for any template, and poll the queue: no message arrives for any stack event (create, update, update-rollback, delete).
3. UpdateStack with NotificationARNs=[] then with NotificationARNs=[topicArn] again: no transition in the (absent) stream is observable.
4. CreateStack with an AWS::CloudFormation::CustomResource whose ServiceToken is an SNS topic: no request envelope is published to that topic, so no ResponseURL can be answered with {"Status":"FAILED","Reason":...} and the stack never reaches ROLLBACK_COMPLETE.
