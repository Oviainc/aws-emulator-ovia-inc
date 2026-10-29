Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes
Issue-Key: config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item|_await_history_items#7|Timeout|time.sleep(10)
Issue-Key: config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes|_await_history_items#11|Timeout|time.sleep(10)
Issue-Commit: 969cd59636329c2a8786028d0edb680658e5b91d

# config__sqs: a running allSupported recorder still produces no AWS::SQS::Queue configuration item, so neither the live-attribute echo nor the ResourceDeleted item ever appears (1 cause)

> _Note: this issue was identified against the code base at commit `969cd59636329c2a8786028d0edb680658e5b91d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
ONE root cause, grounded in 2 failing behavioral checks of this replay.

Cause 1 of 1: the configuration recorder records nothing for an SQS queue, so the AWS::SQS::Queue configuration-item history never exists.
AWS: Recorded behavior only (the Config model describes the reads, not the recording): with a customer-managed configuration recorder whose recordingGroup is {allSupported: true} running and a delivery channel in place, creating an SQS queue causes Config to discover it and write a configuration item for resourceType AWS::SQS::Queue, so GetResourceConfigHistory for that queue's resource id returns at least one configurationItem whose `configuration` echoes the queue's live attributes (the values GetQueueAttributes reports) and whose arn/resourceName identify the queue; deleting the queue then appends a further configuration item with configurationItemStatus ResourceDeleted, so the history holds the create-then-delete sequence.
Emulator: no configuration item is ever produced for the queue. Both checks poll the history read for the queue's item until the harness's 60 s cap and never see one, so neither the attribute echo nor the ResourceDeleted transition can be observed.
Why: a client using Config as the inventory/audit record of its SQS fleet sees an account with no queues — no current configuration to read, no deletion event to react to, and any rule, aggregator or snapshot derived from AWS::SQS::Queue items is permanently empty.
Fix: the recording pass that discovers resources of other services for a running allSupported recorder and writes their configuration items into Config's state (including the ResourceDeleted item on deletion), which GetResourceConfigHistory / BatchGetResourceConfig / SelectResourceConfig then report.
Traces: config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item, config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes (both "Check timed out after 60.0s"). Open issues 20261025T144945Z-config-the-configuration-recorder-never-records-resources-from.md and 20261029T065729Z-config-sqs-the-stopped-recorder-calibration-never-gets.md describe the same recorder gap for OTHER checks and carry neither of these two replay keys; these two checks must be re-replayed to confirm the fix.

## Evidence
Replay output, config__sqs service:

[mismatch] config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes — Error: Check timed out after 60.0s. The check creates an SQS queue with a running {allSupported: true} recorder plus delivery channel and awaits a history item for the queue so it can compare the item's `configuration` to the live GetQueueAttributes values; no configuration item ever appears.

[mismatch] config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item — Error: Check timed out after 60.0s. The check awaits the create-time configuration item and then the follow-on item with configurationItemStatus ResourceDeleted after DeleteQueue; neither arrives.

Expected in both: GetResourceConfigHistory(resourceType=AWS::SQS::Queue, resourceId=<queue id>) returns configurationItems (the first echoing the queue's live attributes, the last carrying ResourceDeleted after deletion). Actual: the history stays empty for the whole poll window.

## Steps to Reproduce
1. Against the emulator, create an IAM role Config can assume, an S3 delivery bucket with the AWSConfig* bucket policy, then put-configuration-recorder with recordingGroup {allSupported: true, includeGlobalResourceTypes: false}, put-delivery-channel, start-configuration-recorder.
2. sqs create-queue, note the queue's resource id/name.
3. Poll config get-resource-config-history --resource-type AWS::SQS::Queue --resource-id <queue> — configurationItems stays empty (no item is ever written).
4. sqs delete-queue and keep polling — no configurationItem with configurationItemStatus ResourceDeleted appears either.
