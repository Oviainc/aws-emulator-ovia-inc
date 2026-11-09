Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-agentcore__cloudwatch.active-session-count-reaches-cloudwatch-per-service-dimension
Issue-Key: bedrock-agentcore__cloudwatch.active-session-count-reaches-cloudwatch-per-service-dimension|_poll_until#13|AssertionError|raise AssertionError(
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# bedrock-agentcore__cloudwatch: AWS/Bedrock-AgentCore ActiveSessionCount is never published, so the per-Service session gauge reads 0 forever

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMetrics

## Description
Cause 1 of 1: starting Bedrock AgentCore sessions mints no AWS/Bedrock-AgentCore ActiveSessionCount series, so the gauge that reports live sessions per built-in tool never leaves 0.

AWS: AgentCore built-in tools publish the AWS/Bedrock-AgentCore namespace; ActiveSessionCount is dimensioned on {Service} with values such as AgentCore.CodeInterpreter / AgentCore.Browser, and it rises to the number of sessions a caller currently holds open. Recorded behavior only (the metric is documented, not modelled in Smithy): the recording shows at least 2 for Service=AgentCore.CodeInterpreter while two sessions are open.

Emulator: after the check starts its sessions, GetMetricStatistics for AWS/Bedrock-AgentCore ActiveSessionCount with Dimensions=[{Service=AgentCore.CodeInterpreter}] reads 0.0 for the whole 480 s budget — the counter is published (or defaulted) but never incremented by session start, or no series is minted at all.

Why: a client cannot see concurrency at all. Session-capacity dashboards read flat zero, and any alarm on ActiveSessionCount never fires, so the quota/health signal AgentCore users scale on is silently dead.

Fix: the AgentCore session lifecycle (start/stop session) state component, which must increment and decrement the AWS/Bedrock-AgentCore ActiveSessionCount series dimensioned on Service as sessions open and close.

Traces: bedrock-agentcore__cloudwatch.active-session-count-reaches-cloudwatch-per-service-dimension.

## Evidence
Expected: AWS/Bedrock-AgentCore ActiveSessionCount, Service=AgentCore.CodeInterpreter >= 2 in the metric minute in which two sessions are open.
Actual: 'AssertionError: AWS/Bedrock-AgentCore/ActiveSessionCount for Service=AgentCore.CodeInterpreter reported at least 2 in the window starting <minute>: not observed within 480.0s; last saw 0.0'.

## Steps to Reproduce
1. Start two AgentCore code-interpreter sessions and keep them open.
2. Poll cloudwatch GetMetricStatistics(Namespace='AWS/Bedrock-AgentCore', MetricName='ActiveSessionCount', Dimensions=[{'Name':'Service','Value':'AgentCore.CodeInterpreter'}], Statistics=['Maximum'], Period=60).
3. Observe the series stays 0.0 while both sessions are open.
