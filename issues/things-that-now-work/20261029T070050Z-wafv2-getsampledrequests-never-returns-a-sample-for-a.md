Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: wafv2.e2e-logging-configuration-capture-flow
Issue-Shape-Id: com.amazonaws.wafv2#AWSWAF_20190729
Issue-Key: wafv2.e2e-logging-configuration-capture-flow|_poll_for_samples#23|TimeoutError|raise TimeoutError("sampled requests appearing: the run budget ended first; not graded")
Issue-Commit: f9dfc7a2b81347ee1edf7acfb92674e6dbacd756

# wafv2 GetSampledRequests never returns a sample for a matched rule with SampledRequestsEnabled=true on a web ACL associated with a REST stage (1 cause)

> _Note: this issue was identified against the code base at commit `f9dfc7a2b81347ee1edf7acfb92674e6dbacd756`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSampledRequests

## Description
Cause 1 of 1: traffic driven through an API Gateway stage fronted by a REGIONAL web ACL whose Priority 0 Count rule matches the request (and whose VisibilityConfig.SampledRequestsEnabled is true) never becomes retrievable through GetSampledRequests, so the end-to-end logging/sampling flow cannot proceed past that leg.
Failing check: wafv2.e2e-logging-configuration-capture-flow

AWS: a REGIONAL web ACL associated with a REST stage inspects each request, and a rule whose VisibilityConfig has SampledRequestsEnabled=true makes matching requests retrievable through GetSampledRequests for that rule's MetricName within the requested TimeWindow. Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json): GetSampledRequests input WebAclArn (required, @length 20..2048, pattern \\S), RuleMetricName (required, @length 1..255, pattern ^[\\w#:\\.\\-/]+$), Scope (required enum), TimeWindow (required) and MaxItems (required, @range 1..500); output SampledRequests (list of SampledHTTPRequest) and PopulationSize (default 0). SampledHTTPRequest requires Request (HTTPRequest, 'detailed information about the request') and Weight, and carries Action ('The action that WAF applied to the request'), Timestamp, RuleNameWithinRuleGroup, RequestHeadersInserted, ResponseCodeSent and Labels. Modeled errors: WAFInvalidParameterException and WAFNonexistentItemException (client, httpError 400), WAFInternalErrorException (server, 500). For a Count-action rule the returned sample's Action is 'COUNT' and its Request.Headers reproduce the request's headers — here the marker single header the ByteMatchStatement matched EXACTLY.

Emulator: the sampling store is empty forever. PutLoggingConfiguration and GetLoggingConfiguration round-trip the S3 destination (those legs pass), ten GETs carrying the marker header are driven through the associated stage, and then the replay detail is TimeoutError: 'sampled requests appearing: the run budget ended first; not graded', raised from the check's _poll_for_samples, which re-drives matching traffic on every iteration. So GetSampledRequests keeps answering with an empty SampledRequests list for a rule that both matched the traffic and has SampledRequestsEnabled=true, and the later legs — the delivered S3 log record and the post-DeleteLoggingConfiguration WAFNonexistentItemException — are never reached. Hypothesis: nothing records an inspected request, because the associated stage is not running WAF evaluation at all (the same edge-enforcement gap filed for the apigateway__wafv2 and cloudwatch__wafv2 tokens).

Why: GetSampledRequests is the only per-request evidence a client has of what a web ACL actually did. An emulator that accepts the web ACL, the association and the logging configuration with 200s and then returns an empty sample list tells a client that inspection is on while leaving no audit trail, no per-rule attribution and no way to distinguish 'no traffic matched' from 'nothing is wired up'. Tooling that waits for a sample before asserting a rule works hangs instead of failing, and a rule that never matched is indistinguishable from one that matched every request.

Fix: the wafv2 request-sampling state and the GetSampledRequests handler that reads it, behind the web-ACL enforcement hook on the associated API Gateway stage — an inspected request matching a rule with SampledRequestsEnabled=true must be recorded and returned by GetSampledRequests keyed by that rule's MetricName and bounded by the requested TimeWindow and MaxItems, with Action reflecting the rule's action ('COUNT' for a Count rule), Weight set, Timestamp inside the window and Request.Headers reproducing the request headers.

Related open issue: 20261026T023047Z-wafv2-logging-surface-putloggingconfiguration-accepts-a-destination-whose.md lists this check id in its Issue-Trace-Ids and describes the 'no sampled request and no delivered log record' behaviour as its Cause 2, but the suite marks this check neither OWNED nor ALREADY FILED — its recorded replay key is 'wafv2.e2e-logging-configuration-capture-flow|_poll_for_samples#23|Timeout|time.sleep(interval_s)' while the check now fails at _poll_for_samples line 316 with an explicit TimeoutError, so the key no longer matches and the failure is not gated by it. It is filed here, grounded in the current replay, and that issue is named rather than duplicated silently; a fixer closing both should treat them as the same sampling/enforcement gap.

Traces: wafv2.e2e-logging-configuration-capture-flow (error).

## Evidence
[error] wafv2.e2e-logging-configuration-capture-flow
  File "check_wafv2_e2e_logging_configuration_capture_flow.py", line 103, in run
    samples = _poll_for_samples(
  File "...", line 316, in _poll_for_samples
    raise TimeoutError("sampled requests appearing: the run budget ended first; not graded")
  TimeoutError: sampled requests appearing: the run budget ended first; not graded

Setup that did succeed in the same run: CreateWebACL (Scope=REGIONAL, DefaultAction Allow, one Priority 0 Count rule with ByteMatchStatement EXACTLY on SingleHeader 'x-simparity-sample' = 'sample-me', SampledRequestsEnabled=true, MetricName 'CatchAllCountMetric'); AssociateWebACL to the deployed stage ARN; PutLoggingConfiguration with LogDestinationConfigs=['arn:aws:s3:::aws-waf-logs-<token>'] echoed back; GetLoggingConfiguration round-tripping the same destination.

Expected (AWS): after ten GETs to the stage invoke URL carrying 'x-simparity-sample: sample-me', GetSampledRequests(WebAclArn=<acl arn>, RuleMetricName='CatchAllCountMetric', Scope=REGIONAL, TimeWindow={last 15 minutes}, MaxItems=<n>) returns a non-empty SampledRequests list containing at least one SampledHTTPRequest whose Action == 'COUNT', whose Weight is present, whose Timestamp lies inside the window and whose Request.Headers include the marker header, with PopulationSize reflecting the inspected population.
Actual (emulator): SampledRequests stays empty on every poll while fresh matching traffic is re-driven, so no sample is ever produced for a matched rule with sampling enabled; the dependent S3 log-record leg and the post-DeleteLoggingConfiguration WAFNonexistentItemException leg are never reached.

## Steps to Reproduce
1. apigateway: CreateRestApi (REGIONAL), GET on the root resource with a MOCK integration and a 200 method/integration response, CreateDeployment to a stage; note the invoke URL and the stage ARN arn:aws:apigateway:<region>::/restapis/<api-id>/stages/<stage>.
2. s3 CreateBucket(Bucket='aws-waf-logs-<token>').
3. wafv2 CreateWebACL(Scope=REGIONAL, DefaultAction={Allow:{}}, VisibilityConfig metrics+sampling enabled) with one rule: Priority 0, Action={Count:{}}, Statement={ByteMatchStatement:{SearchString:'sample-me', FieldToMatch:{SingleHeader:{Name:'x-simparity-sample'}}, PositionalConstraint:'EXACTLY', TextTransformations:[{Priority:0,Type:'NONE'}]}}, VisibilityConfig={SampledRequestsEnabled:true, CloudWatchMetricsEnabled:true, MetricName:'CatchAllCountMetric'}.
4. wafv2 AssociateWebACL(WebACLArn=<acl arn>, ResourceArn=<stage arn>), retrying WAFUnavailableEntityException.
5. wafv2 PutLoggingConfiguration(LoggingConfiguration={ResourceArn:<acl arn>, LogDestinationConfigs:['arn:aws:s3:::aws-waf-logs-<token>']}) and GetLoggingConfiguration — both round-trip.
6. GET the stage invoke URL about ten times with header 'x-simparity-sample: sample-me'.
7. wafv2 GetSampledRequests(WebAclArn=<acl arn>, RuleMetricName='CatchAllCountMetric', Scope=REGIONAL, TimeWindow={StartTime: now-15m, EndTime: now}, MaxItems=100) — observe SampledRequests is empty on every call instead of returning a sample with Action 'COUNT' and the marker header in Request.Headers.
8. Or replay wafv2.e2e-logging-configuration-capture-flow.
