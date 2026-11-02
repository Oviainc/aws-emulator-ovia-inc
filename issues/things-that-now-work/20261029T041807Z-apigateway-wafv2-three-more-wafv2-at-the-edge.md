Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin apigateway__wafv2.wafv2-custom-response-body-and-status-served-at-the-edge-for-a-blocked-request apigateway__wafv2.wafv2-scope-down-statement-limits-a-managed-rule-group-to-a-uri-prefix
Issue-Key: apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-custom-response-body-and-status-served-at-the-edge-for-a-blocked-request|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-scope-down-statement-limits-a-managed-rule-group-to-a-uri-prefix|_poll#16|Timeout|time.sleep(interval_s)
Issue-Commit: d7344a303bafe2b6a404f5912cec42ec875cddf1

# apigateway__wafv2: three more WAFv2-at-the-edge checks hang to the 60 s cap — a web-ACL-associated REST stage never answers, so CustomRequestHandling inserts, a Block CustomResponse and a managed-rule ScopeDownStatement are all unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `d7344a303bafe2b6a404f5912cec42ec875cddf1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: a deployed API Gateway REST stage fronted by a WAFv2 regional web ACL never completes an HTTP request, so every behaviour that depends on WAF actually evaluating a request at the edge is unreachable. Three checks that are NOT covered by any existing issue's replay key fail this way: the Count-action CustomRequestHandling header insert, the Block-action CustomResponse, and the managed-rule-group ScopeDownStatement scoping. Failing check ids: apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin, apigateway__wafv2.wafv2-custom-response-body-and-status-served-at-the-edge-for-a-blocked-request, apigateway__wafv2.wafv2-scope-down-statement-limits-a-managed-rule-group-to-a-uri-prefix.

AWS: a request to a deployed REST stage always terminates with a bounded HTTP response, and when a regional web ACL is associated WAF evaluates it before the integration runs.
- CustomRequestHandling (Smithy: CustomRequestHandling.InsertHeaders is required, a CustomHTTPHeaders list with @length min 1, usable on a non-blocking action such as Count): for a MATCHING request the origin sees the configured header renamed with the fixed prefix 'x-amzn-waf-' prepended to the configured name (configured name 'x-waf-inserted' arrives as 'x-amzn-waf-x-waf-inserted'; the value is the configured value verbatim), the bare configured name is NOT present, and the status is unchanged (200, because Count is non-terminating). A non-matching request to the same stage under the same ACL carries neither form. GetSampledRequests then reports the same insertion (Smithy: SampledHTTPRequest.RequestHeadersInserted, and Action == 'COUNT').
- Block + CustomResponse (Smithy: CustomResponse.ResponseCode required, @range 200-599; CustomResponseBodyKey references a key in the web ACL's CustomResponseBodies map; ResponseHeaders is a CustomHTTPHeaders list): the matched path is served WAF's synthesised response — the configured ResponseCode (not API Gateway's default 403 envelope), the referenced CustomResponseBodies entry byte-exact as the body, and each configured ResponseHeader added — and the origin is never reached. A sibling path that does not match keeps getting 200 from the origin and does NOT carry the custom response header.
- ManagedRuleGroupStatement.ScopeDownStatement (Smithy: ScopeDownStatement is an optional nested Statement that 'narrows the scope of the web requests that are evaluated by the managed rule group'): with a scope-down on UriPath CONTAINS '/api/', an attack payload on a path inside the predicate is refused 403, the identical payload on a path outside it is served 200 by the origin, a benign request inside the predicate is served 200, and after UpdateWebACL removes only the scope-down the previously out-of-scope payload is refused 403 too.
The edge status codes themselves are recorded behavior only (execute-api is not Smithy-modelled); what is structural is that the stage always terminates a request with an HTTP response.

Emulator: all three checks are killed by the harness at its 60 s cap; the detail is exactly 'Error: Check timed out after 60.0s'. No status line and no body ever arrives from the stage invoke URL — not even on the pre-WAF baseline leg each check runs first (each polls the plain MOCK integration for a 200 before attaching the ACL, with the harness-level DATAPLANE_UNIMPLEMENTED marker reserved for 'nothing answered'). Consequently no inserted header, no synthesised custom response, no scope-down decision, and no GetSampledRequests sample is ever produced.

Why: a client fronted by WAF gets no response at all where AWS serves a 200, a 403 or the configured custom status, so it cannot tell allowed traffic from refused traffic and any per-request timeout budget is blown. Because the dependent legs never run, an emulator that inserted the bare header name instead of the 'x-amzn-waf-'-prefixed one, that served the origin's body instead of the CustomResponseBodies bytes, or that ignored ScopeDownStatement and evaluated a managed rule group against every path would be indistinguishable from a correct one.

Fix: the execute-api data plane and its WAFv2 inspection hook — a request to a deployed REST stage must be terminated with an HTTP response, and when a web ACL is associated it must be evaluated first: non-terminating actions apply CustomRequestHandling.InsertHeaders (prefixing each configured name with 'x-amzn-waf-') and pass the request to the integration, Block with CustomResponse synthesises the response from CustomResponse/CustomResponseBodies instead of calling the integration, and a ManagedRuleGroupStatement with a ScopeDownStatement only evaluates requests matching that nested statement. The sampled-request recording that GetSampledRequests reads sits behind the same hook.

Related open issues that do NOT own these three checks (the suite marks none of them OWNED or ALREADY FILED): 20261025T092822Z-apigateway-wafv2-six-wafv2-at-the-edge-checks.md, 20261025T081755Z-apigateway-wafv2-six-more-wafv2-at-the-edge.md and 20261023T210711Z-apigateway-wafv2-all-eleven-wafv2-at-the-edge.md.

Traces: apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin, apigateway__wafv2.wafv2-custom-response-body-and-status-served-at-the-edge-for-a-blocked-request, apigateway__wafv2.wafv2-scope-down-statement-limits-a-managed-rule-group-to-a-uri-prefix.

## Evidence
All three failing checks report the identical failure mode:
  [mismatch] apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-custom-response-body-and-status-served-at-the-edge-for-a-blocked-request — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-scope-down-statement-limits-a-managed-rule-group-to-a-uri-prefix — Error: Check timed out after 60.0s

Expected (AWS):
- Count + CustomRequestHandling: matching request -> HTTP 200 and the origin receives header 'x-amzn-waf-<configured name>' with the configured value, and no header under the bare configured name; non-matching request -> HTTP 200 with neither header; GetSampledRequests returns a sample with RequestHeadersInserted naming 'x-amzn-waf-<configured name>' with that value and Action 'COUNT'.
- Block + CustomResponse: matched path -> the configured ResponseCode with the referenced CustomResponseBodies content as the exact body and each configured ResponseHeader present, and no origin body; sibling unmatched path -> HTTP 200 from the origin with that header absent; GetWebACLForResource still reports the associated ACL and GetWebACL still stores the body map.
- ScopeDownStatement: payload inside the predicate -> 403; identical payload outside -> 200 from the origin; benign request inside -> 200; after removing only the scope-down, the out-of-scope payload -> 403.

Actual (emulator): no HTTP response of any kind arrives from the stage invoke URL within the harness's 60 s cap in any of the three checks; every check dies at the cap before a WAF decision, custom response, inserted header or sample can be observed.

## Steps to Reproduce
1. apigateway CreateRestApi (REGIONAL), add a MOCK-integration resource whose response template echoes $input.params(...) / $context.resourcePath, then CreateDeployment to a stage.
2. GET the stage invoke URL — observe nothing answers (the check is killed at the 60 s harness cap), where AWS serves HTTP 200 with the template body.
3. wafv2 CreateWebACL (Scope=REGIONAL) with, in turn: (a) a ByteMatchStatement rule whose Action is Count with CustomRequestHandling.InsertHeaders [{Name:'x-waf-inserted',Value:<v>}]; (b) a UriPath ByteMatchStatement rule whose Action is Block with CustomResponse {ResponseCode:418, CustomResponseBodyKey:'denied', ResponseHeaders:[{Name:'x-waf-probe',...}]} plus CustomResponseBodies={'denied':{ContentType:APPLICATION_JSON,Content:<json>}}; (c) a ManagedRuleGroupStatement (AWS / AWSManagedRulesSQLiRuleSet) with a ScopeDownStatement of UriPath CONTAINS '/api/'.
4. wafv2 AssociateWebACL with the stage ARN (retrying WAFUnavailableEntityException).
5. Send the matching and non-matching requests described in Evidence — none returns, so no inserted header, no 418 custom body/header, and no in-scope/out-of-scope 403/200 split is observable; GetSampledRequests never reports a sample either.
6. Or replay the three check ids in trace_ids.
