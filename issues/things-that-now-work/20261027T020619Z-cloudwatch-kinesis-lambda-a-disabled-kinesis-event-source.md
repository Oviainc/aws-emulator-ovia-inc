Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog
Issue-Key: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog|run#13|Timeout|time.sleep(10)
Issue-Commit: 9a356a0438a215c6e535efb212b208d427610e62

# cloudwatch__kinesis__lambda: a disabled Kinesis event source mapping accumulates no observable backlog and no IteratorAge is ever metered after it is re-enabled (1 cause)

> _Note: this issue was identified against the code base at commit `9a356a0438a215c6e535efb212b208d427610e62`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateEventSourceMapping

## Description
One net-new root cause, grounded in one failing behavioral check (Cause 1 of 1).
AWS: recorded behavior only (neither the Lambda nor the CloudWatch model states metric emission). On AWS, a Kinesis event source mapping that is disabled while records are put to the stream leaves those records in the shard; when the mapping is enabled again, Lambda consumes the aged batch and publishes AWS/Lambda IteratorAgeMilliseconds for the mapping, so the metric for the held-back batch is high (here: records held ~90 s, i.e. above one CloudWatch period) while records put after the mapping is already enabled meter a fresh, low age.
Emulator: the check makes no progress and is cut off by the suite's per-check deadline — `Error: Check timed out after 60.0s`. No invocation of the backlog batch and no IteratorAgeMilliseconds datapoint is observable, so the aged-vs-fresh distinction can never be asserted. Hypothesis: Kinesis event source mappings are never polled at all, so neither the backlog delivery nor the derived metric exists; the open issues 20261027T001720Z-dynamodb-kinesis-lambda-a-kinesis-stream-event-source.md and 20261025T035202Z-cloudwatch-kinesis-lambda-createeventsourcemapping-never-authorizes-the-function.md record neighbouring Kinesis-ESM causes (delivery and authorization) but neither carries this replay key; what is new here is the metric obligation: IteratorAgeMilliseconds must be emitted and must reflect how long the batch sat in the shard.
Why: a client that monitors consumer lag on a Kinesis-triggered function sees no IteratorAgeMilliseconds at all, so lag alarms never fire and never clear, and a backlog that built up while a mapping was disabled is indistinguishable from no backlog.
Fix: the Lambda event-source-mapping poller for Kinesis sources (it must stop consuming while the mapping is Disabled and consume the accumulated records once Enabled) and the CloudWatch metric emission that must publish AWS/Lambda IteratorAgeMilliseconds for the delivered batch, dimensioned by the function.
Traces: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog.

## Evidence
Report detail for cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog: `Error: Check timed out after 60.0s`.
Expected: with the Kinesis ESM disabled, 5 backlog records are put to the stream and held ~90 s; after UpdateEventSourceMapping --enabled, the function is invoked with those records and GetMetricData/GetMetricStatistics on AWS/Lambda IteratorAgeMilliseconds reports an age at least as large as the hold (minus a small clock skew), while 3 records put afterwards meter an age below a fresh-record ceiling.
Actual: no delivery of the held-back batch and no IteratorAgeMilliseconds datapoint is observable; the check cannot proceed and is cut off.
Caveat for the fixer: this scenario deliberately holds records for ~90 s, so it needs wall-clock room beyond the suite's 60 s per-check deadline; the finding is the absent delivery and absent metric, not the cut-off itself.

## Steps to Reproduce
Create a Kinesis stream and a Lambda returning the record count; `aws lambda create-event-source-mapping --event-source-arn <stream-arn> --function-name <fn> --starting-position LATEST --enabled false` (or disable it after creation); PutRecords 5 records; wait ~90 s; `aws lambda update-event-source-mapping --uuid <id> --enabled`; PutRecords 3 more records; then `aws cloudwatch get-metric-statistics --namespace AWS/Lambda --metric-name IteratorAgeMilliseconds --dimensions Name=FunctionName,Value=<fn> --statistics Maximum --period 60 ...`. Observe no invocation of the held batch and no IteratorAgeMilliseconds datapoints. Replay: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog.
