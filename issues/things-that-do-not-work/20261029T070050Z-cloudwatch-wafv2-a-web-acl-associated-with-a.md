Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions
Issue-Key: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions|_poll_until_steady#16|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: f9dfc7a2b81347ee1edf7acfb92674e6dbacd756

# cloudwatch__wafv2: a web ACL associated with a REST stage still does not enforce — GET /quiet never returns 403, so no AWS/WAFV2 BlockedRequests/CountedRequests series under WebACL/Rule/Region is ever reachable (1 cause)

> _Note: this issue was identified against the code base at commit `f9dfc7a2b81347ee1edf7acfb92674e6dbacd756`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: with a REGIONAL web ACL associated to a deployed API Gateway REST stage, the stage never produces the enforcement outcome the ACL's rules dictate, so the WAF telemetry that depends on inspected requests is never driven and never readable from CloudWatch. The check's enforcement gate — a steady window of 403s from GET <invoke_url>/quiet, the path matched by a Block rule whose VisibilityConfig.CloudWatchMetricsEnabled is false — never opens, and the poll ends with the runner's budget rather than the condition.
Failing check: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions

AWS: a REGIONAL web ACL associated with a REST stage inspects every request before the integration runs and the stage answers with a bounded HTTP response — 403 for a path a terminating Block rule matches (including a Block rule with CloudWatchMetricsEnabled=false: the flag governs telemetry, never enforcement), 200 for a path matched only by a Count-action rule (Count is non-terminating). WAF then publishes to the CloudWatch namespace AWS/WAFV2 the metrics BlockedRequests and CountedRequests, carrying the WebACL, Rule and Region dimensions together, with Sums equal to the exact number of requests each rule acted on; a rule whose VisibilityConfig.CloudWatchMetricsEnabled is false publishes no datapoint at all while still enforcing. Smithy (services/service-com-amazonaws-wafv2-awswaf-20190729/smithy-model/com-amazonaws-wafv2-awswaf-20190729.json): AssociateWebACL input WebACLArn + ResourceArn (both required, @length 20..2048, pattern \\S), modeled errors WAFUnavailableEntityException / WAFInvalidParameterException / WAFNonexistentItemException / WAFInvalidOperationException / WAFLimitsExceededException / WAFFeatureNotIncludedInPricingPlanException (client, httpError 400) and WAFInternalErrorException (server, 500); VisibilityConfig.CloudWatchMetricsEnabled and MetricName are the modeled switches for this emission. The edge status codes and the AWS/WAFV2 namespace and dimension names are recorded behavior only (execute-api and the WAF metric pipeline are not Smithy-modelled); what is structural is that an associated web ACL decides each request and that a metrics-enabled rule produces a series a customer can read back.

Emulator: the enforcement gate never opens. The replay detail is TimeoutError: 'web ACL <acl name> taking effect on stage arn:aws:apigateway:<region>::/restapis/<api-id>/stages/prod: GET /quiet returning 403: the run budget ended first; not graded', raised from the check's _poll_until_steady. AssociateWebACL itself is accepted (the association retry loop completed), so the emulator reports the ACL as governing the stage while the stage does not act on it. Because the gate never opens, none of the 7 blocked / 3 counted / 5 quiet-blocked requests is driven and no AWS/WAFV2 series is ever queried: the dimension set (WebACL + Rule + Region together), the exact Sums, the Unit 'None' on the datapoints and the 'metrics off publishes nothing' twin are all unverified. Hypothesis: the execute-api data plane has no WAFv2 inspection hook, so a web-ACL-associated stage answers from its integration as if no ACL existed.

Why: a client fronted by WAF on an API Gateway stage is served traffic AWS refuses — an over-permissive failure that makes a negative security test pass against the emulator — and an operator gets no telemetry. Without a series carrying WebACL + Rule + Region together a customer cannot tell which of several rules acted on their traffic, and CloudWatchMetricsEnabled cannot be distinguished from decoration: publishing nothing, publishing only a per-web-ACL total, and publishing regardless of the flag are all indistinguishable today.

Fix: the execute-api data plane's WAFv2 inspection hook — a request to a stage with an associated web ACL must be evaluated against the ACL's rules in priority order and terminated accordingly (403 on a terminating Block regardless of that rule's CloudWatchMetricsEnabled, the integration's 200 when only a Count rule matches) — and behind it the AWS/WAFV2 metric publisher, which must emit BlockedRequests/CountedRequests under the WebACL/Rule/Region dimensions for rules with CloudWatchMetricsEnabled=true and nothing for rules with it false.

Related open issue that the suite marks neither OWNED nor ALREADY FILED for this check: 20261025T092153Z-cloudwatch-wafv2-no-aws-wafv2-metric-is-ever.md records this same check when the associated stage produced no HTTP response at all and the harness killed the check at its 60 s cap; it does not carry this check's current replay key. The observed mode has shifted (the poll now reports the enforcement condition specifically, rather than the check dying with no status line), so this is filed as the current, grounded divergence and that issue is named here rather than duplicated silently.

Traces: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions (error).

## Evidence
[error] cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions
  File "check_cloudwatch__wafv2_blocked_and_counted_requests_carry_the_webacl_and_rule_dimensions.py", line 105, in run
    _poll_until_steady(
  File "...", line 414, in _poll_until_steady
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: web ACL <acl name> taking effect on stage arn:aws:apigateway:<region>::/restapis/<api-id>/stages/prod: GET /quiet returning 403: the run budget ended first; not graded

Expected (AWS), with a REGIONAL web ACL (DefaultAction Allow) carrying three ByteMatchStatement/UriPath rules — priority 0 '/blocked' Block with CloudWatchMetricsEnabled=true, priority 1 '/counted' Count with CloudWatchMetricsEnabled=true, priority 2 '/quiet' Block with CloudWatchMetricsEnabled=false — associated with the deployed stage:
  - GET <invoke_url>/quiet -> HTTP 403 steadily (the metrics flag does not affect enforcement)
  - GET <invoke_url>/blocked -> HTTP 403; GET <invoke_url>/counted -> HTTP 200 (Count is non-terminating)
  - cloudwatch GetMetricStatistics Namespace=AWS/WAFV2, MetricName=BlockedRequests, Dimensions WebACL=<acl>/Rule=<blocked rule metric>/Region=<region> settles at Sum 7; CountedRequests for the counted rule's metric settles at Sum 3; datapoints carry Unit 'None'
  - ListMetrics reports a dimension set containing WebACL, Rule and Region together
  - the quiet rule's metric name yields no datapoint for either metric, though its 403s are already proven
Actual (emulator): AssociateWebACL succeeds, but GET /quiet never answers 403 — the enforcement window never opens, so no request volume is driven and CloudWatch is never read. Every metric assertion (namespace, dimension set, Sums 7/3, Unit, metrics-off silence) is unreached.

## Steps to Reproduce
1. apigateway CreateRestApi (REGIONAL) with a {proxy+} resource and an ANY MOCK integration returning 200; CreateDeployment to stage 'prod'.
2. wafv2 CreateWebACL(Scope=REGIONAL, DefaultAction={Allow:{}}) with three ByteMatchStatement/UriPath/ENDS_WITH rules: priority 0 '/blocked' Action Block, VisibilityConfig.CloudWatchMetricsEnabled=true; priority 1 '/counted' Action Count, CloudWatchMetricsEnabled=true; priority 2 '/quiet' Action Block, CloudWatchMetricsEnabled=false — each with its own MetricName; web-ACL VisibilityConfig metrics enabled.
3. wafv2 AssociateWebACL(WebACLArn=<acl arn>, ResourceArn=arn:aws:apigateway:<region>::/restapis/<api-id>/stages/prod), retrying WAFUnavailableEntityException until it succeeds.
4. GET <invoke_url>/quiet repeatedly — observe the emulator never answers 403 (enforcement never takes effect), where AWS refuses every such request with 403.
5. Had it enforced: drive 7 GETs to /blocked, 3 to /counted, 5 to /quiet, then cloudwatch GetMetricStatistics on Namespace AWS/WAFV2 for BlockedRequests/CountedRequests under Dimensions WebACL/Rule/Region, and ListMetrics for that WebACL; confirm Sums 7 and 3, Unit 'None', and no datapoint for the quiet rule's metric.
6. Or replay cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions.
