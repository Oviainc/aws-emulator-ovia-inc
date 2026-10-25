Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged
Issue-Key: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected|_poll#19|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers|_poll#12|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged|_poll#12|Timeout|time.sleep(interval_s)
Issue-Commit: ffece56e68e804f0450b8a83661b5d243139afbd

# apigateway__wafv2: six more WAFv2-at-the-edge checks hang to the 60 s cap — a web-ACL-associated REST stage never answers, so no block, sample or reference refusal is observable (1 cause)

> _Note: this issue was identified against the code base at commit `ffece56e68e804f0450b8a83661b5d243139afbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: once a WAFv2 web ACL is associated with a deployed API Gateway stage, requests to that stage never complete, so every behaviour that depends on an inspected request — managed-rule blocks, rate-based keys, forwarded-IP matching, scope-down limiting, and the reference/lock-token refusals that are asserted to leave the edge unchanged — is unreachable.

AWS: a web ACL associated with a REST stage inspects every request and the stage returns a bounded HTTP response: 403 when a rule blocks (the SQLi managed rule group, the forwarded-IP set reference at the configured header position, the rate-based rule after its limit, the managed group only within the scope-down URI prefix) and 200 when it does not; GetSampledRequests then names the terminating rule for a blocked request (Smithy: GetSampledRequests returns SampledRequests with the rule name, errors WAFInvalidParameterException/WAFNonexistentItemException at 400). The control-plane refusals these checks pair with the edge are modeled: DeleteRegexPatternSet and DeleteWebACL both declare WAFAssociatedItemException (httpError 400) for an entity still referenced by a web ACL rule or still associated with a resource, and WAFOptimisticLockException (httpError 400) for a stale LockToken; after such a refusal the edge must still serve the unchanged ACL. Edge behaviour itself is recorded behavior only (execute-api is not in a Smithy model), but each of these six checks must first get a response from the stage.

Emulator: all six checks are killed by the harness at its 60 s cap ('Check timed out after 60.0s'); the request to the associated stage never returns, so no status, no block, no sample and no post-refusal re-check is ever produced.

Why: a client fronted by WAF gets no response at all instead of 200/403, so it cannot distinguish allowed from refused traffic; and because the 'leaves the edge unchanged' assertions can never run, an emulator that wrongly deleted a referenced regex pattern set or an associated web ACL would be indistinguishable from one that refused correctly.

Note: the existing open issue 20261023T210711Z-apigateway-wafv2-all-eleven-wafv2-at-the-edge.md owns three other checks of this group; these six are not marked OWNED or ALREADY FILED, so they are filed here with their own replay keys.

Fix: the execute-api data plane's WAFv2 inspection path — a request to a stage with an associated web ACL must be evaluated and terminated with an HTTP response (403 on a terminating block action, 200 otherwise) instead of being left open; the sampled-request recording and the WAFAssociatedItemException / WAFOptimisticLockException refusal paths in the wafv2 control plane sit behind it.

Traces: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected, apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected, apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position, apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule, apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers, apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged.

## Evidence
All six checks report the same failure mode:
  [mismatch] apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers — Error: Check timed out after 60.0s
  [mismatch] apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged — Error: Check timed out after 60.0s

Expected (AWS): the associated stage answers each request — 403 when a rule terminates, 200 otherwise; GetSampledRequests names the terminating rule; DeleteRegexPatternSet / DeleteWebACL refuse with HTTP 400 WAFAssociatedItemException while referenced/associated and with HTTP 400 WAFOptimisticLockException for a stale LockToken, and the edge keeps serving the unchanged ACL afterwards.
Actual (emulator): no HTTP response from the stage within the 60 s harness cap in any of the six checks.

## Steps to Reproduce
1. Create a REST API with a working method/integration and deploy it to a stage.
2. wafv2 CreateWebACL (Scope=REGIONAL) with the rule under test (managed SQLi rule group; or a rate-based rule; or an IPSet reference with ForwardedIPConfig at a header position; or a managed group with a scope-down URI-prefix statement; or a rule referencing a RegexPatternSet), then AssociateWebACL with the stage ARN.
3. Send a request to the stage invoke URL that the rule should block (and one it should allow).
4. Observe that no response ever arrives (harness kills each check at 60 s).
5. Consequently the paired control-plane legs are never exercised: DeleteRegexPatternSet while referenced, DeleteWebACL while associated, and UpdateWebACL with a stale LockToken — each expected to fail with HTTP 400 (WAFAssociatedItemException / WAFOptimisticLockException) and leave the edge serving unchanged.
