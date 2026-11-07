Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sns__sqs.xsvc-cloudformation-3-notification-arns-cleared-stops-readded-resumes-delete-notifies cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-notification-arns-cleared-stops-readded-resumes-delete-notifies|_drain_until#16|AssertionError|raise AssertionError(
Issue-Key: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old|get#17|AssertionError|raise AssertionError(
Issue-Commit: 8662809dae52673eae9c053e761f1b93b4d6fc67

# cloudformation__sns__sqs: 2 net-new causes — a stack's NotificationARNs deliver no stack-event notification, and an SNS-backed custom resource is sent Create+Delete instead of the stack update's Update

> _Note: this issue was identified against the code base at commit `8662809dae52673eae9c053e761f1b93b4d6fc67`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack/UpdateStack NotificationARNs; UpdateStack with a Custom:: resource over an SNS ServiceToken

## Description
This issue carries 2 net-new root causes.

Cause 1 of 2: a stack created with NotificationARNs publishes nothing to the named SNS topic — not even its own CREATE_COMPLETE — so the "clear the ARNs and delivery stops, re-add them and it resumes, delete notifies" behavior cannot be exercised past its positive control.
Failing check ids for this cause: cloudformation__sns__sqs.xsvc-cloudformation-3-notification-arns-cleared-stops-readded-resumes-delete-notifies.
AWS: recorded behavior only — the notification envelope is not described by the CloudFormation Smithy model (the surface is CreateStack/UpdateStack NotificationARNs). The recording shows CloudFormation publishing one SNS message per stack and resource status transition to every ARN in NotificationARNs, each message body being the key='value' line form (StackId, StackName, LogicalResourceId, PhysicalResourceId, ResourceStatus, ResourceType, Timestamp, ClientRequestToken, Namespace, ResourceProperties); an UpdateStack that omits NotificationARNs clears them and stops delivery; a later UpdateStack that supplies them again resumes it; and DeleteStack notifies.
Emulator: the subject stack's own CREATE_COMPLETE never arrives — the check fails on its positive control with "queue never yielded [the subject's own CREATE_COMPLETE notification (positive control on this queue)] within 150.0s; collected []". The SQS queue subscribed to the topic stays empty, so nothing about clearing or restoring the ARNs can be observed and the negative phases pass vacuously.
Why: the only push notification channel CloudFormation offers for stack progress is silent. Operators and automation subscribed to a stack's topic learn nothing about creates, updates, rollbacks or deletes, and a consumer cannot distinguish "notifications were correctly suppressed" from "notifications are never sent".
Fix: the CloudFormation stack-operation event publisher — for every stack and resource status transition, publish the key='value' notification body to each ARN currently in the stack's NotificationARNs, and honour an UpdateStack that clears or re-supplies that list.

Cause 2 of 2: an UpdateStack over a Custom:: resource whose ServiceToken is an SNS topic sends the provider a Create followed immediately by a Delete instead of the update's Update request, so the "new PhysicalResourceId on update triggers a cleanup Delete of the old one" sequence is wrong.
Failing check ids for this cause: cloudformation__sns__sqs.xsvc-cloudformation-3-sns-custom-resource-new-physical-id-on-update-triggers-cleanup-delete-of-old.
AWS: recorded behavior only — the custom-resource provider protocol is not in the Smithy model. The recording shows the ordering: stack CREATE sends the provider one request with RequestType "Create"; an UpdateStack that changes the resource's properties sends RequestType "Update" carrying the old PhysicalResourceId (plus OldResourceProperties); only if the provider answers that Update with a *different* PhysicalResourceId does CloudFormation then send a RequestType "Delete" naming the superseded old physical id, as the cleanup phase after the update succeeds.
Emulator: the provider's backlog for the stack holds [('Create', 'Thing'), ('Delete', 'Thing')] and no Update ever arrives — the check reports "no provider request [first Update] arrived within 150.0s; backlog held [('Create', 'Thing'), ('Delete', 'Thing')]". The update is therefore driven as a replace-by-delete rather than as an Update the provider can answer, so the provider never gets the chance to mint a new physical id and the cleanup Delete is not the documented post-update cleanup of a superseded id.
Why: custom-resource providers are written against the Create/Update/Delete contract: an Update that is delivered as a Delete destroys the externally managed object the provider is supposed to mutate in place, loses OldResourceProperties, and makes the physical-id-change cleanup semantics (the only way a provider signals replacement) impossible to implement correctly.
Fix: the CloudFormation custom-resource provisioner's update path — for a resource that already exists, send RequestType Update with the current PhysicalResourceId and OldResourceProperties, and emit the cleanup Delete for the old physical id only after the provider's Update response returns a different PhysicalResourceId.

Traces: the two check ids in trace_ids, one per cause.

The open issues 20261029T183635Z-cloudformation-sns-sqs-a-stack-s-notificationarns-never.md and 20261105T141713Z-cloudformation-sns-sqs-4-net-new-root-causes.md carry other checks in this group; neither carries either of these two replay keys.

## Evidence
Cause 1 — Expected: the stack's own CREATE_COMPLETE notification (key='value' body carrying StackId/StackName/ResourceStatus/ClientRequestToken) is published to each ARN in NotificationARNs and visible on the topic's SQS subscriber. Actual: AssertionError: queue never yielded [the subject's own CREATE_COMPLETE notification (positive control on this queue)] within 150.0s; collected [].

Cause 2 — Expected: provider requests in order Create, then Update (carrying the old PhysicalResourceId and OldResourceProperties) for the UpdateStack, then — only because the Update response returned a new PhysicalResourceId — a Delete naming the superseded old id. Actual: AssertionError: no provider request [first Update] arrived within 150.0s; backlog held [('Create', 'Thing'), ('Delete', 'Thing')].

## Steps to Reproduce
Cause 1: create an SNS topic with an SQS subscription; CreateStack with NotificationARNs=[topic] over a template with one AWS::SSM::Parameter driven by a stack parameter; drain the queue for the stack's CREATE_COMPLETE notification. Then UpdateStack without NotificationARNs (delivery must stop), UpdateStack with them again (must resume) and DeleteStack (must notify).
Cause 2: create an SNS topic with an SQS subscription; CreateStack with a Custom::Versioned resource whose ServiceToken is that topic; answer the Create request; then UpdateStack changing the resource's Name property and read the provider's next request — it is a Delete, not an Update.
Or replay the two check ids in trace_ids.
