Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged
Issue-Key: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected|_poll#20|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position|_poll#17|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers|run#17|Timeout|time.sleep(2.0)
Issue-Key: apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged|_poll#13|Timeout|time.sleep(interval_s)
Issue-Commit: dd185e64193b12242704f82dcc412d70cd33b9eb

# apigateway__wafv2: six WAFv2-at-the-edge checks still hang to the 60 s cap — a web-ACL-associated REST stage never answers, so no block, no sample and no reference/lock-token refusal is observable (1 cause)

> _Note: this issue was identified against the code base at commit `dd185e64193b12242704f82dcc412d70cd33b9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: once a WAFv2 web ACL is associated with a deployed API Gateway REST stage, requests to that stage never complete. Every behaviour that depends on an inspected request — a managed-rule block, a rate-based key, forwarded-IP matching by header position, the sampled request that names the terminating rule, and the 'leaves the edge unchanged' leg of the control-plane refusals — is therefore unreachable.

AWS: a web ACL associated with a REST stage inspects every request and the stage always returns a bounded HTTP response — 403 when a rule's action terminates the request (the managed SQLi rule group on a SQLi payload, the IPSet reference evaluated at the configured forwarded-IP header position, the rate-based rule once its limit is crossed) and 200 when no rule terminates it; after the rate window the same caller is served again. GetSampledRequests then returns the sampled request with the terminating rule's name (Smithy: GetSampledRequests requires WebAclArn/RuleMetricName/Scope/TimeWindow/MaxItems and models WAFInvalidParameterException and WAFNonexistentItemException at httpError 400, WAFInternalErrorException at 500). The paired control-plane refusals are modeled: DeleteRegexPatternSet and DeleteWebACL each declare WAFAssociatedItemException (httpError 400) — for a regex pattern set still referenced by a web ACL rule, and for a web ACL still associated with a protected resource — and WAFOptimisticLockException (httpError 400) for a stale LockToken; after such a refusal the edge must still serve the unchanged ACL. The edge statuses themselves are recorded behavior only (execute-api is not in a Smithy model); what is structural is that a request to a deployed stage always terminates with an HTTP response.

Emulator: all six checks are killed by the harness at its 60 s cap — detail is exactly "Error: Check timed out after 60.0s". No HTTP status line and no body ever arrives from the associated stage, so no WAF decision, no sampled request, and none of the post-refusal re-checks is ever produced.

Why: a client fronted by WAF gets no response at all where AWS serves a 200 or a 403, so it cannot distinguish allowed from refused traffic and any request-level timeout budget is blown. Because the 'edge unchanged' assertions can never run, an emulator that wrongly deleted a referenced RegexPatternSet, deleted a still-associated WebACL, or accepted a stale LockToken would be indistinguishable from one that refused correctly with HTTP 400.

Fix: the execute-api data plane's WAFv2 inspection path — a request to a stage with an associated web ACL must be evaluated and terminated with an HTTP response (403 on a terminating block action, 200 otherwise) instead of being left open; the sampled-request recording and the WAFAssociatedItemException / WAFOptimisticLockException refusal paths in the wafv2 control plane sit behind it. Related open issues that do NOT own these six checks: 20261023T210711Z-apigateway-wafv2-all-eleven-wafv2-at-the-edge.md (the suite marks only three other checks of that group OWNED by it) and 20261025T081755Z-apigateway-wafv2-six-more-wafv2-at-the-edge.md (its recorded replay keys no longer match these failures, so none of the six is marked OWNED or ALREADY FILED).

Traces: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected, apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected, apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position, apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule, apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers, apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged.

## Evidence
All six failing checks report the identical failure mode:
  [mismatch] apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged — Error: Check timed out after 60.0s

Expected (AWS): the associated stage answers every request — 403 when a rule terminates it, 200 otherwise; GetSampledRequests names the terminating rule for the blocked request; DeleteRegexPatternSet while referenced and DeleteWebACL while associated are refused with HTTP 400 WAFAssociatedItemException, an update with a stale LockToken with HTTP 400 WAFOptimisticLockException, and the edge keeps serving the unchanged ACL afterwards.
Actual (emulator): no HTTP response from the stage within the harness's 60 s cap in any of the six checks; the dependent WAF decision, sample and refusal legs never run.

## Steps to Reproduce
1. Create a REST API with a working method/integration and deploy it to a stage.
2. wafv2 CreateWebACL (Scope=REGIONAL) with the rule under test: the AWSManagedRulesSQLiRuleSet managed rule group; or a RateBasedStatement; or an IPSetReferenceStatement with ForwardedIPConfig at a header position; or a rule referencing a RegexPatternSet.
3. wafv2 AssociateWebACL with the stage ARN (retrying WAFUnavailableEntityException while the stage settles).
4. Send a request to the stage invoke URL that the rule should block, and one it should allow — observe that neither ever returns (the harness kills the check at 60 s), so no 403/200 and no GetSampledRequests entry is observable.
5. Consequently the paired control-plane legs never run: DeleteRegexPatternSet while referenced by the web ACL rule, DeleteWebACL while associated with the stage, and UpdateWebACL with a stale LockToken — each expected to answer HTTP 400 (WAFAssociatedItemException / WAFOptimisticLockException) and leave the edge serving the unchanged ACL.
6. Or replay the six check ids in trace_ids.
