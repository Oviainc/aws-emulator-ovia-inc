Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin
Issue-Key: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin|_assert_stays#34|Timeout|time.sleep(interval_s)
Issue-Commit: 0dfbc31074f5b401402097bdda2b8f8e59b726c4

# EventBridge Pipes publishes no AWS/EventBridge/Pipes metrics: TargetStageFailed never appears for a pipe whose role cannot write its target, and the healthy twin never materialises its documented explicit zero (1 cause)

> _Note: this issue was identified against the code base at commit `0dfbc31074f5b401402097bdda2b8f8e59b726c4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1 — a running pipe publishes no metrics into the AWS/EventBridge/Pipes CloudWatch namespace, so neither the failing pipe's TargetStageFailed nor the healthy pipe's explicit zero is readable, and the pipe's target is never written either.

AWS: a pipe emits per-pipe metrics into the CloudWatch namespace `AWS/EventBridge/Pipes`, dimensioned on `PipeName` alone for the per-pipe series (AwsAccountId is used only for the account-level Concurrency/Invocations series). When a pipe's execution role lacks the target permission (here sqs:SendMessage on the target queue), every execution fails at the target stage and `TargetStageFailed` accumulates >= 1 for that PipeName, with Unit `None` (the Pipes monitoring table gives Units "None" for its count-like metrics — `Count` is the natural wrong answer). The metric is documented to be 0 when no executions failed, so a healthy pipe on identical wiring materialises the series with Sum 0 rather than omitting it. Delivery of the metric itself is documented as best-effort, so only a floor of >= 1 is a requirement, never an exact count. Smithy: CloudWatch metric emission is not part of the Pipes model (services/service-com-amazonaws-pipes-pipes) — recorded/documented behavior only (the check cites https://docs.aws.amazon.com/eventbridge/latest/userguide/eb-pipes-monitoring.html). AWS returns no non-2xx anywhere in this flow: every API call succeeds and the divergence is an absent metric series, not a refusal.

Emulator: nothing in the AWS/EventBridge/Pipes namespace is ever observable. The check builds two SQS source/target pairs, two IAM roles (one deliberately without sqs:SendMessage on its target), and two SQS-to-SQS pipes with SourceParameters.SqsQueueParameters BatchSize 1 / MaximumBatchingWindowInSeconds 0 and DesiredState RUNNING, sends one probe message to each source, then waits — first for the healthy twin's target to receive the probe, then for GetMetricStatistics(Namespace='AWS/EventBridge/Pipes', MetricName='TargetStageFailed', Dimensions=[{PipeName:<bad pipe>}]) to return any datapoint, then for ListMetrics to catalogue that series. The replay cuts the check off with "Check timed out after 60.0s" and none of those reads ever yields data. Hypothesis: the stall begins at the healthy twin's delivery wait, because the pipe data plane never drains its source at all (see the related issues below); the metric reads behind it are consequently unreached in this run, but no AWS/EventBridge/Pipes series exists for any pipe either.

Why: a client monitoring a pipe has no signal whatsoever. The one way AWS exposes 'the pipe is running but refusing to deliver' is a rising TargetStageFailed under the pipe's own PipeName, and the one way it distinguishes 'nothing failed' from 'nothing ran' is the healthy pipe's explicit 0 — so alarms on TargetStageFailed stay in INSUFFICIENT_DATA forever, a mis-permissioned pipe looks identical to a correctly working one, and messages disappear with no observable error.

Fix: the Pipes execution/data-plane in services/service-com-amazonaws-pipes-pipes must publish its CloudWatch metrics (at minimum the target-stage outcome counters TargetStageFailed/TargetStageSucceeded, Unit `None`, dimensioned PipeName alone, written for every evaluated period including zero-valued ones for a healthy pipe) into the CloudWatch metric store in services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801 so that GetMetricStatistics and ListMetrics serve them.

Related but NOT covering this: open issue 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md (a RUNNING pipe never polls its source or delivers to its target) and 20261027T030720Z-pipes-the-data-plane-never-authorizes-against-the.md (the data plane never authorizes against the pipe's execution role) are prerequisites for this check reaching its assertions, but neither is filed against the AWS/EventBridge/Pipes metric namespace and neither carries this check's replay key. Fixing the runtime without publishing the metrics leaves this check still failing.

Traces: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin.

## Evidence
Replay verdict: [mismatch] cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin — "Error: Check timed out after 60.0s".

Expected (real AWS): after one message is sent to each pipe's source queue, (a) ReceiveMessage on the healthy pipe's target queue returns the probe body carrying the check's marker; (b) GetMetricStatistics(Namespace='AWS/EventBridge/Pipes', MetricName='TargetStageFailed', Dimensions=[{'Name':'PipeName','Value':<failing pipe>}], Period=60, Statistics=['Sum','SampleCount','Maximum']) returns datapoints whose Sum totals >= 1.0 and whose Unit set is exactly {'None'}; (c) ListMetrics(Namespace='AWS/EventBridge/Pipes', Dimensions=[{'Name':'PipeName','Value':<failing pipe>}]) catalogues a series whose dimension set is exactly ('PipeName',); (d) the same GetMetricStatistics for the HEALTHY pipe returns a datapoint with Sum exactly 0.0 and keeps reading 0.0 for the whole hold window; (e) ReceiveMessage on the failing pipe's target queue yields 0 messages.

Actual: none of (a)-(d) is ever satisfied inside the 60 s budget — the healthy pipe's target queue stays empty and no AWS/EventBridge/Pipes datapoint or ListMetrics entry is returned for either pipe. All control-plane calls before that returned 2xx: CreateQueue x4, GetQueueAttributes x4, CreateRole/PutRolePolicy x2, CreatePipe x2 with DesiredState='RUNNING', SendMessage x2. No error code and no non-2xx status is produced; the divergence is the absent metric series (and the absent delivery behind it).

## Steps to Reproduce
1. CreateQueue four queues: bad-source, bad-target, ok-source, ok-target; read each QueueArn via GetQueueAttributes(AttributeNames=['QueueArn']).
2. CreateRole(bad-role) trusting pipes.amazonaws.com, with an inline policy allowing sqs:ReceiveMessage/sqs:DeleteMessage/sqs:GetQueueAttributes on bad-source ONLY (no sqs:SendMessage on bad-target). CreateRole(ok-role) with the same statement plus sqs:SendMessage on ok-target.
3. CreatePipe(Name=failing, RoleArn=bad-role, Source=bad-source-arn, Target=bad-target-arn, SourceParameters={'SqsQueueParameters':{'BatchSize':1,'MaximumBatchingWindowInSeconds':0}}, DesiredState='RUNNING'); CreatePipe(Name=healthy, RoleArn=ok-role, Source=ok-source-arn, Target=ok-target-arn, same SourceParameters, DesiredState='RUNNING').
4. Poll DescribePipe(Name=...)['CurrentState'] on both until 'RUNNING'.
5. SendMessage one identical probe body (carrying a unique marker) to ok-source and to bad-source.
6. Poll ReceiveMessage(QueueUrl=ok-target, MaxNumberOfMessages=10, WaitTimeSeconds=5) -> on AWS the probe arrives carrying the marker; here nothing arrives.
7. Poll GetMetricStatistics(Namespace='AWS/EventBridge/Pipes', MetricName='TargetStageFailed', Dimensions=[{'Name':'PipeName','Value':'failing'}], StartTime=now-25m, EndTime=now+5m, Period=60, Statistics=['Sum','SampleCount','Maximum']) -> expected Datapoints with Sum summing to >= 1.0 and Unit 'None'; actual: Datapoints is empty on every poll.
8. ListMetrics(Namespace='AWS/EventBridge/Pipes', Dimensions=[{'Name':'PipeName','Value':'failing'}]) -> expected one series with dimension set exactly ('PipeName',); actual: Metrics is empty.
9. Repeat step 7 with PipeName='healthy' -> expected Datapoints present with Sum exactly 0.0 (documented as 0 when no executions failed); actual: Datapoints is empty.
10. ReceiveMessage(QueueUrl=bad-target) -> expected 0 messages (this part is vacuously true here, since nothing is delivered anywhere).
