Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch
Issue-Key: bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch|run#15|Timeout|time.sleep(_POLL_S)
Issue-Commit: b4d018646e260fb276f96a884ffb19d3cc9227fb

# Bedrock inference publishes no AWS/Bedrock CloudWatch token metrics: InputTokenCount/OutputTokenCount at ModelId never account for the tokens the inference reported inline

> _Note: this issue was identified against the code base at commit `b4d018646e260fb276f96a884ffb19d3cc9227fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
ONE CAUSE, on the observability edge between bedrock-runtime inference and the CloudWatch metric store.

Cause 1 of 1: a successful Bedrock inference contributes nothing to the AWS/Bedrock CloudWatch metric namespace, so the token metrics an operator alarms on never exist. Failing check: bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch.

AWS: not expressible in Smithy — CloudWatch metrics are not modeled, so this is documented-plus-recorded behavior only. The Amazon Bedrock user guide's runtime-metrics page publishes the namespace AWS/Bedrock with the metrics InputTokenCount and OutputTokenCount and states "Amazon Bedrock supports the following dimensions: ModelId - all metrics", aggregated on a 1-minute period and visible in near real time; the series is additionally discoverable through CloudWatch ListMetrics with exactly the dimension set [{Name: ModelId, Value: <modelId>}]. The inline oracle for the expected magnitude is modeled: Smithy ConverseResponse.usage -> TokenUsage with inputTokens / outputTokens / totalTokens all @required, @range min 0 ("The number of tokens sent in the request to the model" / "The number of tokens that the model generated for the request"). So on AWS, after two Converse calls on one modelId, a CloudWatch GetMetricStatistics(Namespace=AWS/Bedrock, MetricName=InputTokenCount, Dimensions=[ModelId=<modelId>], Statistics=[Sum]) over a window containing those calls sums to at least the inputTokens the two responses reported inline, and the same holds for OutputTokenCount.

Emulator: the emulator serves both Converse calls with HTTP 200 and non-zero usage.inputTokens/outputTokens (so it has the numbers in hand), but the AWS/Bedrock InputTokenCount series never reaches that floor: the check's convergence loop polls GetMetricStatistics every 15s and never saw the sum reach the inline-reported total before the run was cut off at 60s (four polls), where a target that publishes the datapoint at invocation time converges on the first poll within seconds. Hypothesis: the inference handlers never write any datapoint into the emulator's CloudWatch metric store, so AWS/Bedrock is an empty namespace (consistent with it being asserted by no other passing check in this suite).

Why: token metrics are the series cost attribution, capacity planning and TPM-quota alarms are built on. A client or operator config validated against this emulator sees an alarm that silently never fires and a dashboard that stays empty, instead of an error — the most expensive failure shape for observability, because nothing reports that the data is missing. It also makes every downstream Bedrock-observability contract (dimension scoping, datapoint timestamps, ListMetrics discoverability) unreachable.

Fix: the metric-publication step of the bedrock-runtime inference handlers (the Converse/ConverseStream and InvokeModel/InvokeModelWithResponseStream path in services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/src/ops/, the same stage that fills ConverseResponse.usage) must emit the per-invocation datapoints into the CloudWatch emulator's metric state under namespace AWS/Bedrock with the ModelId dimension, so they are readable by GetMetricStatistics and listable by ListMetrics. The published values must be the same token counts the response reports inline, must be dimensioned (a model id that was never invoked must have no series), and must be timestamped at invocation time rather than back-filled.

Traces: bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch.

Note on service attribution: the grounding check's leading token is the composite `bedrock__cloudwatch`, so this issue is filed under that token. The emitting side is bedrock-runtime's inference handler and the consuming side is cloudwatch; bedrock, bedrock-runtime and cloudwatch are all bootstrapped in this checkout, so the per-service gate verdict is FILE for every constituent. No open issue covers this: 20261024T085515Z-bedrock-runtime-converse-conversestream-report-stopreason-max-tokens-on-a.md owns the two bedrock-runtime__cloudwatch checks but only for the stopReason envelope value, and explicitly notes that the AWS/Bedrock metric assertions behind it were never reached.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1, emulator up and answering (bedrock 2/18, bedrock-runtime 15/63 passed, so these calls return 200s rather than being refused).

Failing check: [mismatch] bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch
  Error: Check timed out after 60.0s

What the check does, in order (from the replayed source):
  1. bedrock.GetFoundationModel(modelIdentifier='amazon.nova-lite-v1:0') -> modelDetails.modelId echoed; this passed (no assertion error raised).
  2. Two bedrock-runtime.Converse calls on modelId='amazon.nova-lite-v1:0' with inferenceConfig={'maxTokens': 40, 'temperature': 0.0}. Each asserts usage['inputTokens'] > 0 and usage['outputTokens'] > 0; neither assertion fired, so the emulator returned 200 with non-zero token usage on both.
  3. Convergence loop, every 15s for up to 180s:
       cloudwatch.GetMetricStatistics(Namespace='AWS/Bedrock', MetricName='InputTokenCount',
         Dimensions=[{'Name':'ModelId','Value':'amazon.nova-lite-v1:0'}], StartTime=<window open>,
         EndTime=now+60s, Period=60, Statistics=['Sum'])
       breaks as soon as sum(Datapoints[].Sum) >= usage_one.inputTokens + usage_two.inputTokens.
     The run was still inside this loop when the 60s harness budget expired, i.e. at least four consecutive reads of the AWS/Bedrock InputTokenCount series at ModelId='amazon.nova-lite-v1:0' failed to account for the tokens the two Converse responses had just reported inline.

Expected: the AWS/Bedrock InputTokenCount (and OutputTokenCount) Sum at Dimensions=[ModelId=amazon.nova-lite-v1:0] is >= the inline usage total within the first read or two, and ListMetrics(Namespace='AWS/Bedrock', MetricName='InputTokenCount') surfaces a series whose dimension set is exactly [{'Name':'ModelId','Value':'amazon.nova-lite-v1:0'}].
Observed: the series never reaches the inline floor; no datapoint was ever observed for it inside the window the check opened.

Caveat recorded honestly: the check's own convergence window is 180s and the harness stopped it at 60s, so the 60s figure is the harness budget, not a measured publication latency. It is still the emulator's divergence — an emulator has no real 1-minute aggregation pipeline, so a published datapoint is visible on the first read, and four empty reads over a minute is not explained by propagation.

Model evidence: smithy_query query=shape shape=TokenUsage on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json -> inputTokens/outputTokens/totalTokens @required, @range min 0. CloudWatch metric namespaces are not part of any Smithy model; the AWS/Bedrock namespace, the InputTokenCount/OutputTokenCount metric names and the ModelId dimension come from the Bedrock monitoring-runtime-metrics documentation cited by the check.

## Steps to Reproduce
1. git_clone the repo and let the behavioral suite build and boot the emulator.
2. Re-replay bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch (give it at least the 300s the check was written for, so a slow-but-correct publication path is distinguished from no publication at all).
3. By hand: note the UTC time T, then call Converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[{'text':'Name three colours, comma separated, nothing else.'}]}], inferenceConfig={'maxTokens': 40, 'temperature': 0.0}) twice and add up the two responses' usage.inputTokens and usage.outputTokens.
4. Call cloudwatch GetMetricStatistics(Namespace='AWS/Bedrock', MetricName='InputTokenCount', Dimensions=[{'Name':'ModelId','Value':'amazon.nova-lite-v1:0'}], StartTime=T-60s, EndTime=now+60s, Period=60, Statistics=['Sum']). Expected: Datapoints non-empty and their Sum >= the inline inputTokens total, every Timestamp inside [T-60s, now]. Observed: the sum never reaches the inline total (empty series).
5. Repeat step 4 with MetricName='OutputTokenCount'; expected >= the inline outputTokens total.
6. Call GetMetricStatistics with the same metric but Dimensions=[{'Name':'ModelId','Value':'sim2real.nonexistent-<random>-v1:0'}]: expected NO datapoints, so the fix must be dimensioned per model id and not one aggregate series.
7. Call ListMetrics(Namespace='AWS/Bedrock', MetricName='InputTokenCount') and confirm it surfaces exactly [{'Name':'ModelId','Value':'amazon.nova-lite-v1:0'}] and no series for the never-invoked model id.
