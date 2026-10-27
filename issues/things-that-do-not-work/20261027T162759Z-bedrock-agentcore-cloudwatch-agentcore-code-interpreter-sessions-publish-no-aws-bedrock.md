Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__cloudwatch.active-session-count-reaches-cloudwatch-per-service-dimension
Issue-Key: bedrock-agentcore__cloudwatch.active-session-count-reaches-cloudwatch-per-service-dimension|_poll_until#13|Timeout|time.sleep(interval_s)
Issue-Commit: cb26d64fef7281140067896354f84452321a0aa0

# AgentCore code-interpreter sessions publish no AWS/Bedrock-AgentCore ActiveSessionCount metric to CloudWatch (GetMetricStatistics/ListMetrics stay empty)

> _Note: this issue was identified against the code base at commit `cb26d64fef7281140067896354f84452321a0aa0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCodeInterpreterSession

## Description
Cause 1 of 1: a started-and-READY AgentCore code interpreter session is never reflected as a CloudWatch metric, so the AgentCore -> CloudWatch vended-metric join never completes. Failing check: bedrock-agentcore__cloudwatch.active-session-count-reaches-cloudwatch-per-service-dimension.

AWS: Smithy for the producer side says nothing about metric emission - StartCodeInterpreterSession (model: services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/smithy-model/com-amazonaws-bedrockagentcore-amazonbedrockagentcore.json) models only codeInterpreterIdentifier/sessionId/createdAt in its output, and CodeInterpreterSessionStatus models only READY and TERMINATED, so this is AWS-documented/recorded behavior rather than a modeled contract: AgentCore vends per-service usage metrics into the AWS-owned namespace AWS/Bedrock-AgentCore, metric name ActiveSessionCount, unit Count, dimensioned by Service, with one datapoint per minute per service type, and the Service dimension partitions the count (Service=AgentCore.CodeInterpreter for code-interpreter sessions, Service=AgentCore.Browser for browser sessions). Consumer side is modeled: cloudwatch GetMetricStatistics takes required Namespace/MetricName/StartTime/EndTime/Period plus Dimensions and Statistics and returns Datapoints (each carrying Maximum, Unit and Timestamp); ListMetrics takes Namespace/MetricName/Dimensions and returns Metrics, each with Namespace and Dimensions. So on AWS, while N code-interpreter sessions are READY, GetMetricStatistics(Namespace=AWS/Bedrock-AgentCore, MetricName=ActiveSessionCount, Dimensions=[Service=AgentCore.CodeInterpreter], Period=60, Statistics=[Maximum]) returns at least one datapoint stamped inside the window whose Maximum is >= N and whose Unit is 'Count', ListMetrics lists that exact series, and the Service=AgentCore.Browser series is NOT credited with those sessions.

Emulator: StartCodeInterpreterSession succeeds and mints distinct sessionIds, and GetCodeInterpreterSession reports status READY immediately (services/service-com-amazonaws-bedrockagentcore-amazonbedrockagentcore/src/ops/start_code_interpreter_session.rs, .../src/ops/get_code_interpreter_session.rs both only read/write this service's own state and emit nothing to the metric store). The check then polls GetMetricStatistics for the ActiveSessionCount series and never sees a qualifying datapoint: the poll (480s budget internally) never satisfies its predicate and the check is killed by the harness - 'Check timed out after 60.0s'. The baseline read at the top of the check succeeds and returns 0.0, i.e. GetMetricStatistics answers normally with an empty Datapoints list, so CloudWatch is serving; nothing ever publishes the series. ListMetrics for that namespace/metric/dimension is likewise never reached but would be empty for the same reason. No error code or status diverges here - both GetMetricStatistics and ListMetrics return 200 with an empty result where AWS returns the vended series.

Why: a client that observes AgentCore capacity/usage through CloudWatch - an autoscaler, a dashboard, a CloudWatch alarm on ActiveSessionCount, or any monitoring smoke test - sees a permanently flat zero no matter how many sessions it starts. Alarms never fire, dashboards show no sessions, and code that waits for the metric to appear before proceeding hangs until its own timeout instead of completing. Because the series is also absent from ListMetrics, metric-discovery code concludes AgentCore is not instrumented at all.

Fix: the gap is in the producer's state/session lifecycle and its metric emission into the shared CloudWatch metric store - StartCodeInterpreterSession / StopCodeInterpreterSession (and the TTL-driven termination in the session record's status-as-of logic) must publish the AWS/Bedrock-AgentCore ActiveSessionCount gauge, Unit=Count, dimensioned Service=AgentCore.CodeInterpreter, at minute granularity, so the cloudwatch crate's GetMetricStatistics and ListMetrics handlers serve it; the Browser series must be a separate series so the Service dimension partitions the count rather than one undifferentiated counter being reported under every Service value.

Traces: bedrock-agentcore__cloudwatch.active-session-count-reaches-cloudwatch-per-service-dimension (mismatch, harness timeout at 60s inside the GetMetricStatistics poll).

## Evidence
Expected (AWS): with 2 code-interpreter sessions READY, cloudwatch.get_metric_statistics(Namespace='AWS/Bedrock-AgentCore', MetricName='ActiveSessionCount', Dimensions=[{'Name':'Service','Value':'AgentCore.CodeInterpreter'}], Period=60, Statistics=['Maximum'], StartTime=<minute of readiness>, EndTime=now+1m) returns Datapoints containing at least one entry with Maximum >= 2, Unit == 'Count' and Timestamp >= window start; cloudwatch.list_metrics(Namespace='AWS/Bedrock-AgentCore', MetricName='ActiveSessionCount', Dimensions=[Service=AgentCore.CodeInterpreter]) returns that series with Namespace 'AWS/Bedrock-AgentCore' and exactly the dimension [Service=AgentCore.CodeInterpreter]; the Service=AgentCore.Browser series reports strictly less than that peak.

Actual (emulator): StartCodeInterpreterSession returns 200 with distinct sessionIds and GetCodeInterpreterSession reports status 'READY' for both, but GetMetricStatistics for that namespace/metric/dimension returns 200 with an empty Datapoints list on every poll (the pre-run baseline read returns 0.0 the same way), so the bounded poll for 'AWS/Bedrock-AgentCore/ActiveSessionCount for Service=AgentCore.CodeInterpreter reported at least 2' never succeeds and the check is terminated: 'Error: Check timed out after 60.0s'. No ActiveSessionCount datapoint and no such metric series is ever produced by the AgentCore side.

## Steps to Reproduce
1. Boot the emulator and point botocore at it.
2. cloudwatch.get_metric_statistics(Namespace='AWS/Bedrock-AgentCore', MetricName='ActiveSessionCount', Dimensions=[{'Name':'Service','Value':'AgentCore.CodeInterpreter'}], StartTime=now-5m, EndTime=now, Period=60, Statistics=['Maximum']) -> note the empty Datapoints baseline.
3. Twice: bedrock-agentcore.start_code_interpreter_session(codeInterpreterIdentifier='aws.codeinterpreter.v1', name=<unique>, sessionTimeoutSeconds=900) and record the sessionId.
4. For each sessionId, bedrock-agentcore.get_code_interpreter_session(codeInterpreterIdentifier='aws.codeinterpreter.v1', sessionId=...) until status is a settled value -> both report 'READY'.
5. Repeat the call from step 2 with StartTime = the minute both became READY and EndTime = now+1m, polling for up to several minutes -> Datapoints stays empty; no datapoint with Maximum >= 2 and Unit 'Count' ever appears.
6. cloudwatch.list_metrics(Namespace='AWS/Bedrock-AgentCore', MetricName='ActiveSessionCount', Dimensions=[{'Name':'Service','Value':'AgentCore.CodeInterpreter'}]) -> Metrics is empty.
7. Replay: bedrock-agentcore__cloudwatch.active-session-count-reaches-cloudwatch-per-service-dimension.
