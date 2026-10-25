Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__opensearch__sns__sqs.forged-aws-es-searchable-documents-datapoint-rejected-and-alarm-withholds-notification cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs
Issue-Key: cloudwatch__opensearch__sns__sqs.forged-aws-es-searchable-documents-datapoint-rejected-and-alarm-withholds-notification|_error#12|_Accepted|raise _Accepted(f"the call was accepted, expected a rejection: {result!r}")
Issue-Key: cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs|_wait_alarm_state#14|Timeout|time.sleep(15.0)
Issue-Commit: 7eba44b4c78724d09fd14d928dd44bf5d39abe6e

# CloudWatch accepts PutMetricData into the reserved AWS/ namespace, and alarms never reach ALARM/notify SNS

> _Note: this issue was identified against the code base at commit `7eba44b4c78724d09fd14d928dd44bf5d39abe6e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Two distinct root causes in this service grouping.

Cause 1 of 2: PutMetricData accepts a datapoint in a reserved AWS/* namespace instead of rejecting it.
AWS: PutMetricData refuses a Namespace in the AWS-reserved space with HTTP 400 and error code InvalidParameterValue. Smithy: cloudwatch#PutMetricDataInput$Namespace documents 'To avoid conflicts with Amazon Web Services service namespaces, you should not specify a namespace that begins with AWS/'; the recorded AWS response for Namespace="AWS/ES" is InvalidParameterValue with the message 'The value AWS/ for parameter Namespace is invalid.' (status 400, code InvalidParameterValue are the requirements; the message text is quoted as the recorded example).
Emulator: the call is ACCEPTED — HTTP 200 with an empty JSON body ({} , content-length 2) and no Error member. A forged AWS/ES SearchableDocuments datapoint is therefore writable by any caller.
Why: a client can forge service-owned metric series. Alarms, dashboards and any logic that trusts AWS/* namespaces as service-authored can be driven to arbitrary states by an unprivileged PutMetricData, and code that relies on the 400 to detect a bad namespace sees success instead.
Fix: PutMetricData input validation in the CloudWatch service crate — reject a Namespace beginning with 'AWS/' with ValidationError code InvalidParameterValue / HTTP 400 before any datapoint is stored.
Traces: cloudwatch__opensearch__sns__sqs.forged-aws-es-searchable-documents-datapoint-rejected-and-alarm-withholds-notification.

Cause 2 of 2: a metric alarm driven over its threshold never transitions to ALARM and never publishes to its SNS action, so the alarm->SNS->SQS path never delivers.
AWS: PutMetricAlarm installs an alarm; once datapoints published by PutMetricData satisfy the comparison over the evaluation periods, the alarm's StateValue becomes ALARM, DescribeAlarmHistory records the transition, and each AlarmAction ARN (an SNS topic here) receives a notification whose JSON body carries AlarmName and NewStateValue='ALARM'. Recorded behavior only for the end-to-end timing; the state machine and the notification envelope are the requirement.
Emulator: the check publishes the crossing datapoint repeatedly and polls the alarm and the subscribed SQS queue, and never observes the ALARM state or a notification — the check runs to the harness deadline ('Check timed out after 60.0s') with no progress. Hypothesis: alarm evaluation against stored metric data is not implemented, so StateValue stays INSUFFICIENT_DATA and no AlarmAction is ever fired.
Why: alarm-driven automation is the point of CloudWatch alarms. A client that waits for ALARM, or for the SNS/SQS notification an alarm emits, waits forever; observability and incident-response workflows built on this path cannot be tested or exercised at all.
Fix: the CloudWatch alarm evaluation engine (periodic evaluation of alarm state from stored metric datapoints, state-transition history, and dispatch of AlarmActions/OKActions/InsufficientDataActions to SNS).
Traces: cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs.

## Evidence
Cause 1 — expected: PutMetricData(Namespace='AWS/ES', ...) -> HTTP 400, Error.Code='InvalidParameterValue' (recorded message example: 'The value AWS/ for parameter Namespace is invalid.').
Actual: accepted. _Accepted: the call was accepted, expected a rejection: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.0', 'content-length': '2', ...}}}

Cause 2 — expected: after PutMetricData drives the series past the alarm threshold, DescribeAlarms shows StateValue='ALARM' and the SNS-subscribed SQS queue receives one notification with NewStateValue='ALARM'.
Actual: Error: Check timed out after 60.0s — the alarm state never changes and no notification is delivered.

## Steps to Reproduce
Cause 1:
1. cw.put_metric_data(Namespace='AWS/ES', MetricData=[{'MetricName':'SearchableDocuments','Dimensions':[{'Name':'ClientId','Value':<account>},{'Name':'DomainName','Value':<domain>}],'Value':<threshold>}]).
2. Observe HTTP 200 instead of a 400 InvalidParameterValue.

Cause 2:
1. Create an SNS topic subscribed by an SQS queue.
2. cw.put_metric_alarm(... Namespace=<customer ns>, MetricName='SearchableDocuments', AlarmActions=[topic_arn], threshold crossing comparison ...).
3. Repeatedly cw.put_metric_data into that namespace with a value past the threshold and poll cw.describe_alarms + sqs.receive_message.
4. The alarm never reaches ALARM and no message arrives.

Replay: cloudwatch__opensearch__sns__sqs.forged-aws-es-searchable-documents-datapoint-rejected-and-alarm-withholds-notification, cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs.
