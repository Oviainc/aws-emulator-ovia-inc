Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__wafv2.xsvc-cognito-idp-3-wafv2-block-rule-matching-header-blocked-unmatched-call-allowed
Issue-Key: cognito-idp__wafv2.xsvc-cognito-idp-3-wafv2-block-rule-matching-header-blocked-unmatched-call-allowed|_poll#18|Timeout|time.sleep(interval_s)
Issue-Commit: 2e744e838767d761bb8115d027d1acb9d75669f4

# A web ACL associated with a Cognito user pool never governs the pool's API calls: the check hangs to the 60 s cap

> _Note: this issue was identified against the code base at commit `2e744e838767d761bb8115d027d1acb9d75669f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
Cause 1 of 1: WAF enforcement on cognito-idp user-pool operations is never observed.
AWS: a REGIONAL WAFv2 web ACL associated with a Cognito user pool governs the pool's unauthenticated API operations. With a Block rule matching a request header, InitiateAuth carrying that header is refused at the edge (the call fails rather than reaching the pool) while an otherwise identical call without the header proceeds to the pool's normal outcome; flipping the same rule's Action from BLOCK to COUNT changes enforcement on the LIVE association without re-associating. Recorded behavior only for the per-leg outcomes; what is structural is that every leg produces a response.
Emulator: no leg ever completes — the check exhausts the harness budget and is reported as "Check timed out after 60.0s", so neither the blocked leg, the unmatched-allowed leg, nor the BLOCK->COUNT transition is observable.
Why: the whole WAFv2-in-front-of-Cognito surface is unverifiable; a client sees a hang where AWS either refuses the call at the edge or lets it through.
Fix: the cognito-idp request path when a web ACL is associated with the user pool (web-ACL evaluation on the pool's API calls and response generation) — it must always produce a response.
Traces: cognito-idp__wafv2.xsvc-cognito-idp-3-wafv2-block-rule-matching-header-blocked-unmatched-call-allowed.
Related open issue (does NOT cover this one, it is apigateway-scoped): 20261023T210711Z-apigateway-wafv2-all-eleven-wafv2-at-the-edge.md records the same no-response symptom at an API Gateway stage.

## Evidence
Detail for the check is exactly "Error: Check timed out after 60.0s" — no response is produced for the header-matching InitiateAuth (AWS refuses it under a Block rule), for the unmatched call (AWS allows it), or after switching the rule Action to COUNT.

## Steps to Reproduce
1. Create a Cognito user pool and app client, and a REGIONAL WAFv2 web ACL whose rule Blocks on a request header match.
2. Associate the web ACL with the user pool ARN.
3. Call InitiateAuth with the matching header and then without it; then UpdateWebACL to change the rule Action to COUNT and repeat — none of these legs completes.
4. Or replay cognito-idp__wafv2.xsvc-cognito-idp-3-wafv2-block-rule-matching-header-blocked-unmatched-call-allowed.
