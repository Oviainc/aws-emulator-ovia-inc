Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__opensearch__sns__sqs.forged-aws-es-searchable-documents-datapoint-rejected-and-alarm-withholds-notification cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs
Issue-Key: cloudwatch__opensearch__sns__sqs.forged-aws-es-searchable-documents-datapoint-rejected-and-alarm-withholds-notification|_drain_notifications#26|Timeout|received = sqs.receive_message(
Issue-Key: cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs|_wait_alarm_state#14|Timeout|time.sleep(15.0)
Issue-Commit: 09b4a3c8d695d022b5fa13764068897eb054981c

# cloudwatch__opensearch__sns__sqs: both AWS/ES SearchableDocuments alarm checks hang to the 60 s harness cap — alarms never evaluate into ALARM and the OpenSearch metric/data path never converges (2 causes)

> _Note: this issue was identified against the code base at commit `09b4a3c8d695d022b5fa13764068897eb054981c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Both behavioral checks in the cloudwatch__opensearch__sns__sqs group fail: neither returns at all, each is killed by the harness at the 60 s cap while spinning in a poll loop waiting for emulator state that never arrives. Every other check in this suite that provisions a domain and asserts against it answers within the cap, so the cap is not the limiting factor here — the emulator never produces the state the loop waits for. Two distinct causes, numbered below.

Related but NOT covering: open issue 20261025T020208Z-cloudwatch-alarms-never-evaluate-published-metric-data-no.md ("CloudWatch alarms never evaluate published metric data") and 20261025T020208Z-cloudwatch-sns-sqs-cloudwatch-alarm-actions-never-fire-to-sns-so.md describe the same family of inertness for the plain cloudwatch / cloudwatch__sns__sqs groups; the suite does not mark these two checks as owned by either, and the second cause below (the OpenSearch-emitted AWS/ES series and the domain's own convergence) is outside both.

Cause 1 of 2: a metric alarm over a customer namespace is never evaluated against published metric data, so it never transitions into ALARM and its action never fires.
AWS: Smithy (cloudwatch) — PutMetricAlarm stores the alarm; DescribeAlarms returns MetricAlarm.StateValue of enum StateValue [OK, ALARM, INSUFFICIENT_DATA]; DescribeAlarmHistory returns HistoryItemType StateUpdate items. AWS evaluates a stored alarm on its own clock against the datapoints PutMetricData ingested: with Period=60, EvaluationPeriods=1, Statistic=Maximum, ComparisonOperator=GreaterThanOrEqualToThreshold and Threshold=10.0, a datapoint of 10.0 in the watched namespace/dimensions drives the alarm to ALARM, records a StateUpdate history item, and delivers the configured AlarmActions SNS notification (which, subscribed to an SQS queue, arrives as an SNS envelope whose Message is the alarm notification JSON). No SetAlarmState is used anywhere in the check.
Emulator: in the forged-datapoint check the earlier assertions all pass — PutMetricData into the reserved AWS/ES namespace is correctly refused and the two alarms are created — and the check then enters its positive-control loop, re-putting the identical datapoint into a customer namespace every 15 s and reading DescribeAlarms, for up to 600 s. The check never leaves that loop: the harness kills it at 60 s. The control alarm therefore never reports StateValue=ALARM, so the differential (refused AWS/ES datapoint must NOT drive its alarm, accepted customer-namespace datapoint MUST) can never be observed, and nothing downstream (alarm history StateUpdate, SNS->SQS notification, GetMetricStatistics readback) is reached. Hypothesis: alarms are stored as inert configuration with no evaluation engine, so StateValue never changes from its initial value.
Why: a client that watches a metric and reacts to the alarm — paging, auto-remediation, rollback gating — sees an alarm that stays silent forever against data that breaches its threshold. Worse, against this emulator the negative half of the property (a forged reserved-namespace datapoint must not be able to drive an AWS/ES alarm) is indistinguishable from the positive half, because neither alarm ever fires, so a product cannot tell a working guardrail from an absent one.
Fix: the CloudWatch alarm evaluation path — the component that joins stored alarms to ingested metric datapoints, computes StateValue, appends the StateUpdate history item and dispatches AlarmActions.
Traces: cloudwatch__opensearch__sns__sqs.forged-aws-es-searchable-documents-datapoint-rejected-and-alarm-withholds-notification.

Cause 2 of 2: the end-to-end OpenSearch metric path never converges — a domain's documents never become an AWS/ES SearchableDocuments series, and the check cannot even reach the alarm stage.
AWS: recorded/documented behavior only (the Smithy models carry no metric-emission contract). Per the OpenSearch monitoring documentation, the service publishes AWS/ES SearchableDocuments ("the total number of searchable documents across all data nodes in the cluster") with dimensions ClientId and DomainName; CloudWatch then evaluates an alarm over that series (Statistic=Maximum, Period=60, EvaluationPeriods=1, Threshold=10.0, TreatMissingData=ignore). The recorded sequence is: domain becomes active and its HTTPS endpoint serves, the alarm first observes OK (metric flowing below threshold), 10 documents are indexed over the endpoint, and the alarm then transitions OK -> ALARM with exactly one StateUpdate history item whose stateReason names the threshold, and one SNS notification reaches the subscribed queue with Trigger Namespace=AWS/ES, MetricName=SearchableDocuments and Dimensions {ClientId, DomainName}.
Emulator: the check never returns — the harness kills it at the 60 s cap with no response to report. It cannot have passed any of its graded stages. Hypothesis (the emulator's reply is a timeout, so the stage cannot be pinned from this run): it blocks in one of the poll loops before the alarm assertions — DescribeDomain never reporting the domain searchable / its endpoint never serving, or the AWS/ES series never appearing — and in any case Cause 1 makes the OK and ALARM waits unsatisfiable.
Why: a client that provisions a domain and alarms on its own emitted metrics never gets a usable signal: no AWS/ES series exists to read, DescribeAlarms stays at its initial state, and the SNS/SQS fan-out that production runbooks depend on is silent. Any test of an observability pipeline passes vacuously against this emulator.
Fix: the OpenSearch domain convergence/metric-emission path (publishing the domain's AWS/ES series from cluster contents) plus the CloudWatch alarm evaluation component named in Cause 1.
Traces: cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs.

## Evidence
Suite report (simparity-behavioral-ga-prod-v1), service group cloudwatch__opensearch__sns__sqs: 0/2 (0%), 2 failed, neither marked ALREADY FILED nor OWNED.

[mismatch] cloudwatch__opensearch__sns__sqs.forged-aws-es-searchable-documents-datapoint-rejected-and-alarm-withholds-notification
  Error: Check timed out after 60.0s
  The check's own code reached past its PutMetricData assertions (the reserved-namespace refusal InvalidParameterValue / "The value AWS/ for parameter Namespace is invalid." matched, both alarms created) and hung in:
    while time.monotonic() < deadline:   # deadline = start + 600 s
        cw.put_metric_data(Namespace=control_ns, MetricData=datum)   # Value=10.0, same dimensions
        state = _alarm(cw, control_alarm)["StateValue"]
        if state == "ALARM": break
        time.sleep(15.0)
  Expected: DescribeAlarms reports StateValue="ALARM" for the control alarm (Threshold=10.0, GreaterThanOrEqualToThreshold, Period=60, EvaluationPeriods=1, Statistic=Maximum) and its AlarmActions SNS topic delivers one notification with NewStateValue="ALARM" to the subscribed queue.
  Actual: the loop never breaks within the harness cap — the alarm's StateValue never becomes ALARM, so no notification, no StateUpdate-into-ALARM history item and no differential against the refused AWS/ES datapoint is observable.

[mismatch] cloudwatch__opensearch__sns__sqs.obs-opensearch-2-searchable-documents-metric-crosses-threshold-and-alarm-notifies-sqs
  Error: Check timed out after 60.0s
  Graded sequence that never completed: _create_min_domain -> _wait_searchable -> _wait_data_plane_ready -> PutMetricAlarm(Namespace="AWS/ES", MetricName="SearchableDocuments", Dimensions=[ClientId, DomainName], TreatMissingData="ignore") -> _wait_alarm_state(..., "OK", 600 s) -> PUT index + _bulk 10 docs -> _wait_alarm_state(..., "ALARM", 900 s) -> exactly one StateUpdate OK->ALARM whose stateReason contains "threshold (10.0)" -> one SNS->SQS notification with Trigger {Namespace: "AWS/ES", MetricName: "SearchableDocuments"}.
  Actual: no response at all within the cap; none of the above is reached.

Model citations used: cloudwatch StateValue enum [OK, ALARM, INSUFFICIENT_DATA]; cloudwatch HistoryItemType enum includes StateUpdate and Action.

## Steps to Reproduce
Cause 1:
1. Create an SNS topic and an SQS queue, subscribe the queue to the topic (protocol=sqs, SNS envelope kept).
2. PutMetricAlarm with a unique customer Namespace, MetricName=SearchableDocuments, Dimensions=[{ClientId, <account>}, {DomainName, <any name>}], Statistic=Maximum, Period=60, EvaluationPeriods=1, Threshold=10.0, ComparisonOperator=GreaterThanOrEqualToThreshold, AlarmActions=[<topic arn>].
3. PutMetricData into that namespace with the same dimensions and Value=10.0, repeating every 15 s.
4. Poll DescribeAlarms for the alarm. Expected: StateValue becomes ALARM and DescribeAlarmHistory(HistoryItemType=StateUpdate) records the transition; one notification reaches the queue. Observed: StateValue never becomes ALARM, nothing is delivered.

Cause 2:
1. CreateDomain (OpenSearch_2.11, t3.small.search x1, gp3 10 GiB, account access policy) and wait for it to be active with a serving endpoint.
2. PutMetricAlarm over Namespace=AWS/ES, MetricName=SearchableDocuments, Dimensions=[ClientId, DomainName], Threshold=10.0, TreatMissingData=ignore, AlarmActions=[<topic arn>].
3. Wait for the alarm to read OK, then PUT an index and _bulk-index 10 documents with refresh=true over the domain endpoint.
4. Poll DescribeAlarms. Expected: OK -> ALARM with one StateUpdate naming threshold (10.0) and one SNS->SQS notification whose Trigger is AWS/ES SearchableDocuments. Observed: the sequence never completes — the call chain hangs and the check is killed at the harness cap.
