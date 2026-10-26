Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch
Issue-Key: bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch|run#15|Timeout|time.sleep(_POLL_S)
Issue-Commit: b4d018646e260fb276f96a884ffb19d3cc9227fb

# bedrock__cloudwatch: a Bedrock inference publishes no AWS/Bedrock token metrics, so InputTokenCount/OutputTokenCount never appear at CloudWatch (1 cause)

> _Note: this issue was identified against the code base at commit `b4d018646e260fb276f96a884ffb19d3cc9227fb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
One root cause (Cause 1 of 1).

Cause 1 of 1: Bedrock Runtime inference emits no CloudWatch metric data, so the AWS/Bedrock namespace stays empty after a successful Converse call.
Failing check: bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch

AWS: Amazon Bedrock publishes per-invocation usage metrics into the AWS/Bedrock namespace, dimensioned on ModelId, and the Bedrock monitoring documentation (bedrock/latest/userguide/monitoring-runtime-metrics.html: "Amazon Bedrock supports the following dimensions: ModelId - all metrics") lists InputTokenCount and OutputTokenCount among them, aggregated on a 1-minute period. The oracle for the volume is the API's own inline report: Smithy (bedrock-runtime) ConverseOutput carries `usage: TokenUsage` with required members inputTokens ("The number of tokens sent in the request to the model") and outputTokens ("The number of tokens that the model generated for the request"), so the token counts the service itself reports for a call must be accounted for by that call's contribution to the series. On the read side, Smithy (cloudwatch) GetMetricStatistics returns `Datapoints`, and ListMetrics returns `Metrics` whose `Dimensions` include the series actually in existence; both are implemented in this emulator (they answered, with no NotImplemented). Metric publication itself is not expressible in the Smithy model — this requirement is the documented behavior plus the inline usage report, not a modeled field.

Emulator: after two Converse calls against amazon.nova-lite-v1:0 that are served successfully, GetMetricStatistics(Namespace="AWS/Bedrock", MetricName="InputTokenCount", Dimensions=[{ModelId: <invoked model>}], Period=60, Statistics=["Sum"]) never reaches the token total the two calls reported inline. The check polls that single assertion every 15s and makes no other call in the loop, so it never leaves the loop and the check is killed by the 60s harness cap ("Check timed out after 60.0s"); the earlier legs — GetFoundationModel echoing the modelId, and both Converse calls reporting non-zero usage — had already passed, so the hang is located in the metric convergence and nowhere else. Hypothesis: the invocation path publishes no metric data at all, so the series does not exist and the sum stays 0 forever rather than converging late. Downstream of that same absence, the rest of the check is unreachable: the ListMetrics leg (the series must be surfaced dimensioned on exactly [ModelId=<model>]), the datapoint-timestamp-in-window leg, and the negative leg (a well-formed but never-invoked ModelId must have NO datapoints) are never evaluated.

Why: everything operational that is built on Bedrock token volume silently reports nothing. Capacity planning and cost attribution dashboards render empty, and a TPM-quota or spend alarm over AWS/Bedrock InputTokenCount/OutputTokenCount sits in INSUFFICIENT_DATA and never fires — a client gets no error to react to, only an alarm that never trips, which is the failure mode that looks like "everything is fine". Code that joins an inference to its metered usage after the fact (chargeback, per-model quota tracking) finds no series to join to, even though the invocation returned usage inline.

Fix: the Bedrock Runtime invocation path (Converse / ConverseStream / InvokeModel) must emit metric data into the emulator's CloudWatch metric store at the end of an invocation — one datapoint per call under AWS/Bedrock with the ModelId dimension set to the invoked model id, valued with the same inputTokens/outputTokens the response's `usage` reports (Count unit, 1-minute aggregation), so the existing CloudWatch GetMetricStatistics/ListMetrics read path surfaces it. The emission belongs in the bedrock-runtime invocation handler (with the shared metric-store write used by other emitters), not in the CloudWatch service: GetMetricStatistics and ListMetrics answer correctly here, they simply have nothing to return. Note the dimension must be honoured rather than aggregated away — the check also asserts a never-invoked ModelId has no series. Related but NOT the same cause: open issues record the same "producer publishes no metrics" shape for other producers (20261025T020208Z-cloudwatch-dynamodb-... for AWS/DynamoDB table metrics, 20261024T153105Z-networkmonitor-createprobe-event-propagation for AWS/NetworkMonitor, 20261025T020208Z-cloudwatch-sfn-... for AWS/States); none of them covers AWS/Bedrock, and no open issue names the AWS/Bedrock namespace.

Traces: bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch (mismatch, timed out at the 60s cap inside the InputTokenCount convergence poll).

## Evidence
Expected (AWS): after two Converse calls on amazon.nova-lite-v1:0 reporting inputTokens/outputTokens inline, GetMetricStatistics over Namespace=AWS/Bedrock, MetricName=InputTokenCount, Dimensions=[{Name=ModelId,Value=amazon.nova-lite-v1:0}], Period=60, Statistics=[Sum] returns Datapoints whose Sum is at least the sum of the two reported inputTokens (likewise OutputTokenCount vs outputTokens), every datapoint timestamped inside the window opened by the caller; ListMetrics(Namespace=AWS/Bedrock, MetricName=InputTokenCount) surfaces a metric with Dimensions exactly [{Name=ModelId,Value=amazon.nova-lite-v1:0}]; and the same query for a well-formed but never-invoked ModelId returns an empty Datapoints list.

Actual (emulator): GetFoundationModel echoes the modelId and both Converse calls return non-zero `usage`, but the InputTokenCount sum for that ModelId never reaches the reported total. The check's convergence poll (immediate first probe, then every 15s) never breaks, and the check is killed by the harness: `Error: Check timed out after 60.0s`. No AWS/Bedrock series is observed, so the OutputTokenCount, timestamp-window, dimension-isolation and ListMetrics legs are never reached.

## Steps to Reproduce
1. Boot the emulator and replay bedrock__cloudwatch.bedrock-invocation-publishes-token-metrics-to-cloudwatch.
2. Manually: bedrock.GetFoundationModel(modelIdentifier="amazon.nova-lite-v1:0") to confirm the catalog id.
3. Note the UTC time T, then call bedrock-runtime.Converse(modelId="amazon.nova-lite-v1:0", messages=[{role:user, content:[{text:"Name three colours, comma separated, nothing else."}]}], inferenceConfig={maxTokens:40, temperature:0.0}) twice and record response["usage"]["inputTokens"]/["outputTokens"] from each.
4. Call cloudwatch.GetMetricStatistics(Namespace="AWS/Bedrock", MetricName="InputTokenCount", Dimensions=[{Name:"ModelId",Value:"amazon.nova-lite-v1:0"}], StartTime=T-60s, EndTime=now+60s, Period=60, Statistics=["Sum"]).
   Observed: Datapoints is empty / its Sum never reaches the inline total, no matter how long you poll.
   Expected: a Sum >= the two calls' combined inputTokens.
5. Call cloudwatch.ListMetrics(Namespace="AWS/Bedrock", MetricName="InputTokenCount").
   Observed: the ModelId series is absent. Expected: one metric with Dimensions [{ModelId: amazon.nova-lite-v1:0}], and no series for a model id that was never invoked.
