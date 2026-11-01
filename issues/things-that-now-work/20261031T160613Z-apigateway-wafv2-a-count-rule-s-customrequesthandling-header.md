Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin
Issue-Key: apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin|_poll#29|Timeout|time.sleep(interval_s)
Issue-Commit: 055568fd9ce9a7be471ce8dc9e6772f2075ef53d

# apigateway__wafv2: a Count rule's CustomRequestHandling header insert is still unobservable — the web-ACL-associated REST stage never answers, so no x-amzn-waf- prefixed header and no GetSampledRequests RequestHeadersInserted report (1 cause)

> _Note: this issue was identified against the code base at commit `055568fd9ce9a7be471ce8dc9e6772f2075ef53d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: a deployed API Gateway REST stage fronted by a REGIONAL WAFv2 web ACL never terminates an HTTP request, so a non-terminating (Count) rule's CustomRequestHandling header insertion is never visible to the origin and WAF's own account of it is never produced. The replay report marks this check neither OWNED nor ALREADY FILED, so it is filed here; the open issue 20261029T041807Z-apigateway-wafv2-three-more-wafv2-at-the-edge.md describes the same symptom class for a set of edge checks and does not carry this check's current replay key (its keys name different poll sites), and the open issues 20261029T193654Z-apigateway-wafv2-wafv2-at-the-edge-the-oversize-body-inspection.md, 20261025T081755Z-apigateway-wafv2-six-more-wafv2-at-the-edge.md and 20261029T070050Z-wafv2-getsampledrequests-never-returns-a-sample-for-a.md cover other legs of the WAF-at-the-edge surface. Nothing here reopens a fixed issue.
AWS: a request to a deployed REST stage always terminates with a bounded HTTP response. With a regional web ACL associated to the stage ARN, WAF evaluates the request before the integration runs. For a rule whose Action is Count with CustomRequestHandling.InsertHeaders (Smithy: CustomRequestHandling.InsertHeaders is required, a CustomHTTPHeaders list), a MATCHING request keeps its status (200 — Count is non-terminating) and the origin receives the configured header name with the fixed prefix 'x-amzn-waf-' prepended (configured name 'x-waf-inserted' arrives as 'x-amzn-waf-x-waf-inserted', value verbatim), while the bare configured name is absent; a request that does not match the ByteMatchStatement carries neither form. GetSampledRequests then reports the same insertion (Smithy: SampledHTTPRequest.RequestHeadersInserted, with Action 'COUNT'). The edge status codes themselves are recorded behavior only (execute-api is not Smithy-modelled); what is structural is that the stage terminates every request with a response.
Emulator: the check is killed at the harness's 60 s cap. Replay detail: `Error: Check timed out after 60.0s`. No status line or body arrives from the stage invoke URL — not even on the pre-WAF baseline leg, which polls the plain echoing MOCK integration for a 200 before any ACL is created — so no inserted header, no non-matching control comparison and no sampled-request report is ever produced.
Why: a client behind a WAF-protected stage gets no response where AWS serves 200, so it cannot distinguish allowed from counted traffic and its per-request timeout budget is blown; and because the dependent legs never run, an emulator that inserted the bare configured header name instead of the 'x-amzn-waf-'-prefixed one, inserted it for non-matching requests, or reported a different name/value from GetSampledRequests would be indistinguishable from a correct one.
Fix: the execute-api data plane and its WAFv2 inspection hook — a request to a deployed REST stage must be terminated with an HTTP response, and when a web ACL is associated it must be evaluated first: a non-terminating action applies CustomRequestHandling.InsertHeaders (prefixing each configured name with 'x-amzn-waf-') and then passes the request to the integration, and the sampled-request recording that GetSampledRequests reads sits behind the same hook.
Traces: apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin.

## Evidence
Failing check apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin [mismatch]:
  Error: Check timed out after 60.0s

Expected (AWS):
- Baseline (no ACL): GET <stage>/echo -> 200 with the MOCK template body echoing the origin marker.
- Matching request (header x-waf-trigger: <trigger>): 200, and the origin's echo shows the inserted header under 'x-amzn-waf-x-waf-inserted' with the configured value, and an empty value for the bare 'x-waf-inserted'.
- Control request (no trigger header): 200, with both the prefixed and the bare form absent.
- GetSampledRequests(WebAclArn, RuleMetricName='inserter', Scope=REGIONAL): a sample whose RequestHeadersInserted names exactly 'x-amzn-waf-x-waf-inserted' with that value, and whose Action is 'COUNT'.

Actual (emulator): no HTTP response of any kind arrives from the stage invoke URL; the check is killed at the 60 s harness cap (its internal poll budgets are 180 s for the baseline and 360 s for propagation), so no WAF decision, inserted header or sample is observable.

## Steps to Reproduce
1. apigateway CreateRestApi (REGIONAL) with a MOCK-integration '/echo' resource whose 200 response template echoes $input.params('x-amzn-waf-x-waf-inserted'), $input.params('x-waf-inserted') and $input.params('x-waf-trigger'); CreateDeployment + CreateStage.
2. GET <stage>/echo — observe nothing answers (the check dies at the 60 s harness cap) where AWS serves 200 with the echo body.
3. wafv2 CreateWebACL (Scope=REGIONAL, DefaultAction Allow) with one rule: ByteMatchStatement on SingleHeader 'x-waf-trigger' EXACTLY <trigger>, Action {Count: {CustomRequestHandling: {InsertHeaders: [{Name: 'x-waf-inserted', Value: <value>}]}}}, VisibilityConfig with SampledRequestsEnabled=true.
4. wafv2 AssociateWebACL with the stage ARN (retrying WAFUnavailableEntityException).
5. GET <stage>/echo with and without the x-waf-trigger header — neither returns, so the prefixed/bare header split is unobservable; GetSampledRequests never reports a RequestHeadersInserted entry either.
6. Or replay the check id in trace_ids.
