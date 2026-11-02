Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__sqs.config-batch-get-resource-config-returns-only-the-recorded-key-of-a-mixed-batch config__sqs.config-history-rejects-unrecorded-queue-while-batch-get-omits-it config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes config__sqs.config-select-resource-config-returns-the-recorded-queue-row
Issue-Key: config__sqs.config-batch-get-resource-config-returns-only-the-recorded-key-of-a-mixed-batch|_await_history_items#8|Timeout|time.sleep(2)
Issue-Key: config__sqs.config-history-rejects-unrecorded-queue-while-batch-get-omits-it|_await_history_items#10|AssertionError|raise AssertionError(
Issue-Key: config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item|_await_history_items#7|Timeout|time.sleep(2)
Issue-Key: config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes|_await_history_items#11|Timeout|time.sleep(2)
Issue-Key: config__sqs.config-select-resource-config-returns-the-recorded-queue-row|_await_history_items#6|Timeout|time.sleep(2)
Issue-Commit: 977af42ea79b884a07e35b887b5e587a14d941a9

# config__sqs: a recording configuration recorder never produces a ConfigurationItem for an SQS queue, so history, batch-get, select and the deletion item are all empty

> _Note: this issue was identified against the code base at commit `977af42ea79b884a07e35b887b5e587a14d941a9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResourceConfigHistory

## Description
Cause 1 of 1: with a started recorder whose recording group includes AWS::SQS::Queue, Config records no configuration item for a queue — not on create, not on attribute change, not on delete — so every read surface that depends on a recorded item is empty.

AWS: once StartConfigurationRecorder is called and the recording group covers AWS::SQS::Queue, Config discovers each queue and writes a ConfigurationItem for it. GetResourceConfigHistory then returns items whose configuration echoes the queue's live attributes; BatchGetResourceConfig returns only the recorded keys of a mixed batch (unrecorded ones come back in unprocessedResourceIdentifiers); SelectResourceConfig returns a row per recorded queue; deleting the queue writes a final item with configurationItemStatus ResourceDeleted; and GetResourceConfigHistory for a resource the account never recorded is refused ResourceNotDiscoveredException. Smithy (configservice GetResourceConfigHistory / BatchGetResourceConfig / SelectResourceConfig) models the shapes and ResourceNotDiscoveredException but not the recording itself — the presence of the items is recorded behavior only.

Emulator: the recorder reports recording=true (lastStatus Pending, then Success) but no configuration item ever appears. Four checks exhaust the 60 s budget polling for it, and the fifth fails explicitly: 'the CONTROL configuration item for queue <queue url> ... not met within 0.0s. Last result: None. Recorder status: [{"recording": True, "lastStatus": "Pending", ...}]'. Without any recorded item the refusal for an unrecorded queue is also unprovable: it would only mean the account has nothing recorded at all.

Why: a client using Config as its inventory/compliance source sees an account that records nothing while the recorder claims to be healthy; drift detection, config rules and compliance queries all return empty and look like 'no findings' rather than 'not recording'.

Fix: the configuration-recorder discovery/ingest path for SQS queues — writing the create/update/delete ConfigurationItems that the Config read operations and SelectResourceConfig serve.

Traces: config__sqs.config-batch-get-resource-config-returns-only-the-recorded-key-of-a-mixed-batch, config__sqs.config-history-rejects-unrecorded-queue-while-batch-get-omits-it, config__sqs.config-queue-deletion-recorded-as-resource-deleted-configuration-item, config__sqs.config-recorded-queue-configuration-item-echoes-live-attributes, config__sqs.config-select-resource-config-returns-the-recorded-queue-row. Related open issues that do NOT carry these replay keys: 20261025T144945Z-config-the-configuration-recorder-never-records-resources-from.md, 20261029T065729Z-config-sqs-the-stopped-recorder-calibration-never-gets.md.

## Evidence
Expected: after StartConfigurationRecorder with AWS::SQS::Queue in the recording group, GetResourceConfigHistory returns at least one ConfigurationItem for the queue whose configuration echoes its live attributes; BatchGetResourceConfig returns the recorded key and leaves the unrecorded one unprocessed; SelectResourceConfig returns the queue's row; DeleteQueue adds an item with configurationItemStatus 'ResourceDeleted'.
Actual: no item ever appears. Explicit failure: 'AssertionError: the CONTROL configuration item for queue \'https://<host>/<account>/<queue>\' - without it a refusal for the absent queue would only mean the account has nothing recorded at all not met within 0.0s. Last result: None. Recorder status: [{\'name\': ..., \'recording\': True, \'lastStatus\': \'Pending\', ...}]'. Four sibling checks: 'Error: Check timed out after 60.0s'.

## Steps to Reproduce
1. PutConfigurationRecorder with a recording group including AWS::SQS::Queue and PutDeliveryChannel; StartConfigurationRecorder. 2. CreateQueue; poll GetResourceConfigHistory for the queue's resource id. 3. Also call BatchGetResourceConfig with the recorded queue plus an absent one, SelectResourceConfig for the queue row, and DeleteQueue then poll for the ResourceDeleted item. Replay the five check ids in trace_ids.
