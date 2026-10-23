Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-body-inspection-stops-at-the-oversize-limit-and-oversize-handling-decides-past-it apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin apigateway__wafv2.wafv2-custom-response-body-and-status-served-at-the-edge-for-a-blocked-request apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position apigateway__wafv2.wafv2-label-match-statement-blocks-on-a-label-added-by-a-lower-priority-rule apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers apigateway__wafv2.wafv2-scope-down-statement-limits-a-managed-rule-group-to-a-uri-prefix apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged
Issue-Key: apigateway__wafv2.wafv2-body-inspection-stops-at-the-oversize-limit-and-oversize-handling-decides-past-it|_poll#14|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-custom-request-handling-inserts-prefixed-headers-visible-to-the-origin|_poll#13|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-custom-response-body-and-status-served-at-the-edge-for-a-blocked-request|_poll#15|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-delete-regex-pattern-set-referenced-by-a-webacl-rule-rejected|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-delete-web-acl-associated-with-an-apigateway-stage-rejected|_poll#11|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-forwarded-ip-set-reference-blocks-by-header-position|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-label-match-statement-blocks-on-a-label-added-by-a-lower-priority-rule|_poll#9|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-managed-sqli-rule-group-refuses-a-payload-and-the-sample-names-the-rule|_poll#10|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers|_poll#8|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-scope-down-statement-limits-a-managed-rule-group-to-a-uri-prefix|_poll#16|Timeout|time.sleep(interval_s)
Issue-Key: apigateway__wafv2.wafv2-stale-lock-token-update-rejected-and-leaves-the-edge-unchanged|_poll#8|Timeout|time.sleep(interval_s)
Issue-Commit: 8bb3e31d2bc3a99611966740f58be573e66087fa

# apigateway__wafv2: all eleven WAFv2-at-the-edge checks hang to the 60 s cap at the deployed stage

> _Note: this issue was identified against the code base at commit `8bb3e31d2bc3a99611966740f58be573e66087fa`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDeployment

## Description
Cause 1 of 1: every one of these checks drives a request at a deployed REST API stage with a WAFv2 web ACL associated, and that request never returns.
AWS: the stage answers its invoke URL with the web ACL's decision applied — a custom response body and status served at the edge for a blocked request, custom request handling inserting `x-amzn-waf-` prefixed headers visible to the origin, body inspection stopping at the oversize limit with oversize handling deciding past it, forwarded-IP-set matching by header position, label-match on a label added by a lower-priority rule, a managed SQLi rule group refusing a payload with the rule named in the sample, a rate-based rule refusing then recovering, and a scope-down statement limiting a managed rule group to a URI prefix. The control-plane legs (deleting a regex pattern set still referenced by a rule, deleting a web ACL still associated with a stage, updating with a stale lock token) are refused and leave the edge unchanged. Recorded behavior only for the per-case statuses; what is structural is that every request to a deployed stage produces an HTTP response.
Emulator: no response arrives from the stage. All eleven checks exhaust the harness budget and are reported as "Check timed out after 60.0s", so no WAF decision — and none of the control-plane refusals that are asserted against a live edge — is ever observed.
Why: the whole WAFv2-in-front-of-API-Gateway surface is unverifiable: a client sees a hang where AWS serves an allow, a block, or a custom edge response.
Fix: the execute-api data-plane request path (web-ACL evaluation and response generation at the stage) — it must always produce a response.
Traces: the eleven check ids in trace_ids.

## Evidence
Detail for all eleven checks is exactly "Error: Check timed out after 60.0s" — no HTTP status line and no body ever arrives from the deployed stage.

## Steps to Reproduce
1. Deploy a REST API stage and associate a WAFv2 REGIONAL web ACL with it (blocking rule, custom response, custom request handling, rate-based rule, managed rule group with a scope-down statement).
2. Send allowed and blocked requests to the stage invoke URL and observe that none of them produces a response within 60 s.
3. Or replay the check ids in trace_ids.
