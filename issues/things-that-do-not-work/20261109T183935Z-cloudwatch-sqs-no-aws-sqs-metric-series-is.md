Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__sqs.metrics-sqs-depth-gauges-track-inflight-return-and-delete cloudwatch__sqs.metrics-sqs-dlq-redrive-lands-in-dlq-own-series-not-its-sent-count cloudwatch__sqs.metrics-sqs-never-created-queue-has-no-series-while-idle-queue-reports-zero cloudwatch__sqs.metrics-sqs-putmetricdata-into-aws-sqs-namespace-rejected cloudwatch__sqs.sqs-service-metrics-count-the-real-data-plane-traffic
Issue-Key: cloudwatch__sqs.metrics-sqs-depth-gauges-track-inflight-return-and-delete|_poll_joint#12|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sqs.metrics-sqs-dlq-redrive-lands-in-dlq-own-series-not-its-sent-count|_poll_joint#14|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sqs.metrics-sqs-never-created-queue-has-no-series-while-idle-queue-reports-zero|run#10|Timeout|time.sleep(20.0)
Issue-Key: cloudwatch__sqs.metrics-sqs-putmetricdata-into-aws-sqs-namespace-rejected|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__sqs.sqs-service-metrics-count-the-real-data-plane-traffic|_poll_until_stable#18|Timeout|time.sleep(interval_s)
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudwatch__sqs: no AWS/SQS metric series is ever published — depth gauges, sent/received/deleted counts, delayed messages and DLQ series all stay empty after real data-plane traffic

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: SQS queues publish no AWS/SQS metric series at all, so five checks that drive real data-plane traffic and then read the queue's metrics exhaust their 900 s budget without ever seeing a datapoint.

AWS: every queue publishes the AWS/SQS namespace dimensioned on {QueueName}: the depth gauges ApproximateNumberOfMessagesVisible / ...NotVisible / ...Delayed track the inflight/return/delete lifecycle and a delayed message is counted ONLY in the delayed series; NumberOfMessagesSent / Received / Deleted count the real data-plane calls; a redrive to a dead-letter queue lands in the DLQ's own series rather than inflating its NumberOfMessagesSent; a never-created queue has no series at all while an existing idle queue reports zero. PutMetricData into the service-owned Namespace='AWS/SQS' is refused with 400 InvalidParameterValue so a customer cannot overwrite the series autoscaling scales on, and the genuine series continues to carry only the real traffic. Recorded behavior only for the series content (the metric names are documented, not Smithy shapes).

Emulator: every one of these checks runs out its budget with the AWS/SQS series empty — in the PutMetricData arm the refusal and the custom-namespace positive control both pass, then the read of AWS/SQS ApproximateNumberOfMessagesVisible for the queue never returns the expected depth and the harness kills the check at 900 s. No AWS/SQS datapoint is produced by any queue operation.

Why: SQS is unmonitorable and unscalable. Queue-depth alarms never fire, application-autoscaling policies that target ApproximateNumberOfMessagesVisible never scale, DLQ growth is invisible, and 'queue does not exist' cannot be distinguished from 'queue is idle'. The spoofing negative is also unverifiable: with the genuine series empty, a customer cannot see that the refused value did not land.

Fix: the SQS data-plane accounting component that should emit the AWS/SQS per-queue metric families (depth gauges on queue-state change; sent/received/deleted on each data-plane call; delayed and DLQ series) into the CloudWatch metric store. This is the same family as the open issue 20261029T182452Z-cloudwatch-sqs-sqs-publishes-no-aws-sqs-delayed-message-metric.md, which does not carry these five checks' replay keys.

Traces: cloudwatch__sqs.metrics-sqs-depth-gauges-track-inflight-return-and-delete, cloudwatch__sqs.metrics-sqs-dlq-redrive-lands-in-dlq-own-series-not-its-sent-count, cloudwatch__sqs.metrics-sqs-never-created-queue-has-no-series-while-idle-queue-reports-zero, cloudwatch__sqs.metrics-sqs-putmetricdata-into-aws-sqs-namespace-rejected, cloudwatch__sqs.sqs-service-metrics-count-the-real-data-plane-traffic.

## Evidence
Expected: AWS/SQS ApproximateNumberOfMessagesVisible for the queue peaks at exactly the number of messages sent (e.g. 2 sent, none received), with the depth gauges, sent/received/deleted counts, delayed series and DLQ series all carrying the real traffic.
Actual: no AWS/SQS datapoint is ever read; all five checks end as 'Error: Check timed out after 900.0s'. In the spoof arm the PutMetricData refusal and the custom-namespace control both succeed, proving the metric write path works outside AWS/ while the service-owned series remains empty.

## Steps to Reproduce
1. CreateQueue; SendMessage twice.
2. Poll cloudwatch GetMetricStatistics(Namespace='AWS/SQS', MetricName='ApproximateNumberOfMessagesVisible', Dimensions=[{'Name':'QueueName','Value':<name>}], Period=60, Statistics=['Maximum']).
3. Observe Datapoints == [] indefinitely. Repeat for NumberOfMessagesSent/Received/Deleted, ApproximateNumberOfMessagesDelayed (with DelaySeconds), and a redrive into a dead-letter queue.
