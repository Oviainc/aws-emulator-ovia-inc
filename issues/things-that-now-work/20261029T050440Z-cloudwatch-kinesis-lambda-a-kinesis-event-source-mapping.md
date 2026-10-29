Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog
Issue-Key: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog|run#13|Timeout|time.sleep(10)
Issue-Commit: 96b9d79b1b8395723cfce0b7aa8e9f6fed8726ec

# cloudwatch__kinesis__lambda: a Kinesis event source mapping held Disabled and then enabled emits no AWS/Lambda IteratorAge metric at all, so GetMetricStatistics returns an empty Datapoints list forever and the check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `96b9d79b1b8395723cfce0b7aa8e9f6fed8726ec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause, grounded in the single failing behavioral check that the replay report marked neither ALREADY FILED nor OWNED. The emulator is serving (164/621 checks pass, bootstrap_signal false), so this is a fidelity divergence, not a bootstrap gap. All three services this check touches (cloudwatch, kinesis, lambda) are bootstrapped.

=== Cause 1 of 1: the IteratorAge metric of a Kinesis-sourced event source mapping is never published, so the backlog-age series does not exist ===

AWS: a Lambda event source mapping created with Enabled=false over a Kinesis data stream does not read the stream, so records put on it accumulate in the shard. Smithy (lambda, com-amazonaws-lambda-awsgirapiservice): CreateEventSourceMapping/UpdateEventSourceMapping/GetEventSourceMapping all return EventSourceMappingConfiguration carrying State and StateTransitionReason, and UpdateEventSourceMapping accepts Enabled (boolean) on PUT /2015-03-31/event-source-mappings/{UUID} with success code 202. Once the mapping is flipped to Enabled and reaches State "Enabled", the poller drains the accumulated shard and, for every stream-sourced mapping, Lambda publishes the metric IteratorAge in namespace AWS/Lambda, dimension FunctionName=<function name>, unit Milliseconds, at 1-minute resolution: the age of the oldest record in the batch at the moment the function was invoked. So for a backlog that sat unread for D seconds the first datapoint's Maximum is at least D*1000 ms (minus clock skew), and a later minute of records that are written and read promptly reports a Maximum far below that (here under 20 000 ms and under a quarter of the backlog value). The metric series itself is not described by any Smithy model — CloudWatch models only GetMetricStatistics, whose output is Label plus a Datapoints list — so the IteratorAge semantics above are recorded behavior only, and what the recording asserts is that at least one AWS/Lambda IteratorAge datapoint exists for the function after the drain.

Emulator: no AWS/Lambda IteratorAge datapoint is ever produced for the function. cloudwatch:GetMetricStatistics(Namespace="AWS/Lambda", MetricName="IteratorAge", Dimensions=[{FunctionName:<fn>}], Period=60, Statistics=["Maximum"], Unit="Milliseconds") answers 200 with an empty Datapoints list on every poll, so the check's wait for "IteratorAge datapoint for the drained backlog" never converges and it is cut off by the harness ('Error: Check timed out after 60.0s'). No call is refused anywhere: there is no error code, no non-2xx status and no error message to observe — the divergence is a silent absence of the metric, and the check never reaches either of its two age assertions. Grounded in the replay only, the drain itself is also unobservable: nothing in the emulator's responses shows the backlog being read after Enabled=true, consistent with the Kinesis poller producing neither invocations nor metrics (Hypothesis: the same missing Kinesis poller is the upstream of both, since a metric whose value is the batch's record age can only be emitted by the component that assembles a batch).

Why: a client cannot observe stream-processing lag at all. IteratorAge is the one signal AWS users alarm on to detect a function falling behind its stream, and to size batch/parallelization settings; against this emulator every such alarm stays in INSUFFICIENT_DATA and every dashboard for stream lag is blank. Because the metric is absent rather than wrong, there is nothing for the client to catch or retry — a backlog of any age looks identical to a fully caught-up stream.

Fix: the Kinesis event-source poller in the lambda service crate — the task that reads the mapped stream's shards once the mapping is Enabled and invokes the function with a Kinesis batch — and specifically its metric emission into the CloudWatch metric store (publishing AWS/Lambda IteratorAge with dimension FunctionName at 1-minute resolution in Milliseconds, valued at the age of the oldest record in each delivered batch), together with the mapping's Disabled->Enabled drain path it depends on.

Note on existing issues: OPEN 20261025T035202Z-cloudwatch-kinesis-lambda-createeventsourcemapping-never-authorizes-the-function.md is the same service token but covers CreateEventSourceMapping's missing role authorization; OPEN 20261027T001720Z-dynamodb-kinesis-lambda-a-kinesis-stream-event-source.md covers non-delivery of a Kinesis mapping under a different service token and check; OPEN 20261026T005239Z-dynamodb-kinesis-lambda-a-dynamodb-streams-event-source.md covers a DynamoDB-Streams OnFailure destination. None of them carries this check's replay key and none of them names the AWS/Lambda IteratorAge metric, and the replay report marked this check neither ALREADY FILED nor OWNED.

Traces: cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog.

## Evidence
Replayed suite simparity-behavioral-ga-prod-v1 against the booted emulator: 164/621 passed (26.4%), mismatched 229, errored 228, unloadable 0, bootstrap_signal false. Per-service line: cloudwatch__kinesis__lambda 0/2 (0%) 2 failed — the other one is OWNED by 20261025T035202Z-cloudwatch-kinesis-lambda-createeventsourcemapping-never-authorizes-the-function.md and was skipped.

cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog [mismatch]:
  Error: Check timed out after 60.0s

Expected (real AWS): after kinesis:CreateStream (1 shard, PROVISIONED) reaches ACTIVE, lambda:CreateFunction, and lambda:CreateEventSourceMapping(EventSourceArn=<stream arn>, StartingPosition=TRIM_HORIZON, BatchSize=100, MaximumBatchingWindowInSeconds=0, Enabled=false) reaching State "Disabled", records are put and held unread for the hold window; after lambda:UpdateEventSourceMapping(UUID=..., Enabled=true) reaches State "Enabled" the backlog drains and cloudwatch:GetMetricStatistics(Namespace="AWS/Lambda", MetricName="IteratorAge", Dimensions=[{Name:"FunctionName",Value:<fn>}], Period=60, Statistics=["Maximum"], Unit="Milliseconds") returns at least one datapoint whose Maximum is >= the hold duration in ms (less a small clock-skew tolerance); a later minute of promptly-read records then reports a Maximum well under that first value.

Actual (emulator): GetMetricStatistics answers 200 with Datapoints == [] on every poll for the whole life of the check; the harness's wait for "IteratorAge datapoint for the drained backlog" never satisfies and the check is cut off at the 60 s cap. No call returns an error code, a non-2xx status or an error message — the metric is simply never published, and neither of the check's age assertions is ever evaluated.

## Steps to Reproduce
1. Build and boot the emulator, then replay cloudwatch__kinesis__lambda.obs-lambda-2-kinesis-esm-iterator-age-reflects-disabled-backlog.
2. kinesis:CreateStream(ShardCount=1, StreamModeDetails={StreamMode: PROVISIONED}) and wait for the stream to exist; read its StreamARN via DescribeStreamSummary.
3. iam:CreateRole trusting lambda.amazonaws.com with kinesis:GetRecords/GetShardIterator/DescribeStream/DescribeStreamSummary/ListShards/ListStreams, then lambda:CreateFunction (python3.12 handler returning the record count) and wait for it to become Active.
4. lambda:CreateEventSourceMapping(FunctionName=<fn>, EventSourceArn=<stream arn>, StartingPosition=TRIM_HORIZON, BatchSize=100, MaximumBatchingWindowInSeconds=0, Enabled=false) and poll GetEventSourceMapping until State == "Disabled".
5. kinesis:PutRecords of 5 records, then hold for ~90 s while GetEventSourceMapping keeps reporting State == "Disabled".
6. lambda:UpdateEventSourceMapping(UUID=<uuid>, Enabled=true) and poll until State == "Enabled".
7. Poll cloudwatch:GetMetricStatistics(Namespace="AWS/Lambda", MetricName="IteratorAge", Dimensions=[{Name:"FunctionName",Value:<fn>}], StartTime=<two minutes before the run>, EndTime=now+2min, Period=60, Statistics=["Maximum"], Unit="Milliseconds") -> observe Datapoints is always [], so no backlog age (>= ~85 000 ms) and no later caught-up age (< 20 000 ms) can ever be compared; the check hangs until the harness cap.
