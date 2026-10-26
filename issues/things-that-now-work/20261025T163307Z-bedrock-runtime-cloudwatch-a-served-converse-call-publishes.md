Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension
Issue-Key: bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension|run#12|Timeout|time.sleep(_POLL_S)
Issue-Commit: 2efbe0db58fc669c0e9ee58e639ae2f2fc196447

# bedrock-runtime__cloudwatch: a served Converse call publishes no AWS/Bedrock Invocations/InvocationLatency, so the documented Unit and the ModelId dimension contract are unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `2efbe0db58fc669c0e9ee58e639ae2f2fc196447`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
One net-new root cause for the bedrock-runtime__cloudwatch join. The open issue 20261025T072205Z-bedrock-cloudwatch-a-bedrock-inference-publishes-no-aws.md reports the same shape for the TOKEN metrics (InputTokenCount/OutputTokenCount) under the service token bedrock__cloudwatch; it carries neither this check's id nor its replay key, and it does not cover the Invocations/InvocationLatency series, their Units, or the dimension-isolation requirement. No open issue names the bedrock-runtime__cloudwatch token.

Cause 1 of 1: the Bedrock Runtime invocation path emits no AWS/Bedrock request metrics, so the namespace stays empty after successfully served Converse calls.
Failing check id: bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension.
AWS: metric publication is not expressible in the Smithy model, so this is the documented behavior plus recorded behavior. The Bedrock runtime metrics reference (docs.aws.amazon.com/bedrock/latest/userguide/monitoring-runtime-metrics.html) states 'Invocations | Count | Number of successful requests to the Converse, ConverseStream, InvokeModel, and InvokeModelWithResponseStream API operations' and 'Amazon Bedrock supports the following dimensions: ModelId - all metrics'; InvocationLatency is published in milliseconds. Recorded behavior (the check's own measurement against real AWS, which converged on the first poll in well under 31 s of wall clock): after N served Converse calls against a model id, GetMetricStatistics(Namespace='AWS/Bedrock', MetricName='Invocations', Dimensions=[{Name:'ModelId', Value:<the invoked model id>}], Period=60, Statistics=['Sum','Maximum']) returns datapoints summing to at least this run's own invocation count; the Invocations datapoints carry the CloudWatch StandardUnit value 'Count' and the InvocationLatency datapoints 'Milliseconds', with a positive latency bounded by the elapsed wall time of the run; ListMetrics surfaces the newly published series with Dimensions exactly [{ModelId: <that model id>}]; and a well-formed ModelId dimension value that names no model has NO series at all (ListMetrics never returns it and GetMetricStatistics returns no datapoints), which is what makes the per-model slice real rather than one undifferentiated aggregate.
Emulator: the Converse calls are served (the sibling check in this directory shows Converse returning stopReason 'end_turn'), and the CloudWatch read path answers (GetMetricStatistics/ListMetrics are implemented and return no error), but the Invocations Sum for that ModelId never reaches this run's own floor. The check makes no other call inside its convergence poll, so it never leaves the loop and is killed at the harness cap: 'Error: Check timed out after 60.0s' — well past the sub-31 s convergence the same assertions measured on real AWS. No AWS/Bedrock series is produced at all, so the Unit legs ('Count' for Invocations, 'Milliseconds' for InvocationLatency), the ListMetrics discovery leg and the negative dimension-isolation leg are all unreachable.
Why: every operational view built on Bedrock request volume and latency silently reports nothing. A per-model latency dashboard renders empty, a throttling or latency alarm over AWS/Bedrock InvocationLatency sits in INSUFFICIENT_DATA and never fires (the failure mode that looks like 'everything is fine'), and capacity/cost tooling that slices traffic by ModelId cannot distinguish models. A client gets no error to react to.
Fix: the bedrock-runtime invocation handler (Converse / ConverseStream / InvokeModel / InvokeModelWithResponseStream) must write to the emulator's shared CloudWatch metric store at the end of each invocation — one Invocations datapoint (Unit 'Count') and one InvocationLatency datapoint (Unit 'Milliseconds', the measured server-side duration) per call, under namespace AWS/Bedrock with the ModelId dimension set to the invoked model id, honouring the dimension rather than aggregating it away so a never-invoked ModelId has no series. The emission belongs in the bedrock-runtime service, not in cloudwatch: GetMetricStatistics and ListMetrics answer correctly here and simply have nothing to return.
Traces: bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension.

## Evidence
Replay of simparity-behavioral-ga-prod-v1: bedrock-runtime__cloudwatch 0/2 (0%). This check was not marked ALREADY FILED or OWNED.

bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension [mismatch]: 'Error: Check timed out after 60.0s'. The check issues 3 Converse calls against amazon.nova-lite-v1:0 and then polls _statistics(cw, 'Invocations', <model id>, start, end) = GetMetricStatistics(Namespace='AWS/Bedrock', MetricName='Invocations', Dimensions=[{Name:'ModelId', Value:<model id>}], Period=60, Statistics=['Sum','Maximum']) every 15 s against a declared _CONVERGE_TIMEOUT_S of 210 s, with no other call in the loop.
Expected (AWS, measured by this check's own replay and its blessed sibling: convergence on the first poll, total wall clock under 31 s): Sum >= the 3 invocations this run made; Unit 'Count' on the Invocations datapoints and 'Milliseconds' on the InvocationLatency datapoints with a positive value bounded by the run's elapsed wall time; ListMetrics(Namespace='AWS/Bedrock', MetricName='Invocations') surfacing Dimensions exactly [{ModelId: <model id>}]; and no series whatsoever for a ModelId dimension value this run invented.
Actual (emulator): the Sum never moves off nothing, so the poll runs to the harness kill at 60 s and none of the Unit, ListMetrics or absence legs is ever evaluated.

Not filed from the sibling check in this directory (bedrock-runtime__cloudwatch.refused-invocation-counted-as-client-error-never-as-an-invocation): its first assertion is on generated model text ('No language model runs in this emulator. amazon.nova-lite-v1:0 generated no text for this request.'), a value only a live model produces, so it is not evidence of a metric divergence.

## Steps to Reproduce
1. Boot the emulator; point botocore bedrock-runtime and cloudwatch clients at it.
2. Note the UTC time T, then call bedrock-runtime:Converse(modelId='amazon.nova-lite-v1:0', messages=[{role:'user', content:[{text:'Reply with exactly the single word PONG and nothing else.'}]}], inferenceConfig={maxTokens:32, temperature:0.0}) three times; each returns successfully.
3. cloudwatch:GetMetricStatistics(Namespace='AWS/Bedrock', MetricName='Invocations', Dimensions=[{Name:'ModelId', Value:'amazon.nova-lite-v1:0'}], StartTime=T-60s, EndTime=now+60s, Period=60, Statistics=['Sum','Maximum']).
   Expected: datapoints whose Sum >= 3, each carrying Unit 'Count'. Observed: no datapoints, no matter how long you poll.
4. Same query with MetricName='InvocationLatency'. Expected: datapoints with Unit 'Milliseconds' and a positive value no greater than the wall time the three calls took. Observed: none.
5. cloudwatch:ListMetrics(Namespace='AWS/Bedrock', MetricName='Invocations'). Expected: a metric with Dimensions exactly [{Name:'ModelId', Value:'amazon.nova-lite-v1:0'}]. Observed: absent.
6. Negative control that must keep holding once the fix lands: the same GetMetricStatistics/ListMetrics queries for a ModelId value that names no model must report no series at all (not a zero-valued one).
7. Re-replay: bedrock-runtime__cloudwatch.converse-invocations-reach-cloudwatch-with-the-documented-unit-and-dimension.
