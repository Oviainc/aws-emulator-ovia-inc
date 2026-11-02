Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__wafv2.xsvc-cognito-idp-3-wafv2-block-rule-matching-header-blocked-unmatched-call-allowed
Issue-Key: cognito-idp__wafv2.xsvc-cognito-idp-3-wafv2-block-rule-matching-header-blocked-unmatched-call-allowed|run#22|AssertionError|assert resp["Error"].get("Message") == "Request not allowed due to WAF block.", resp["Error"]
Issue-Commit: afb41014a6bc1e75e8336e677aedbec1633a6643

# cognito-idp__wafv2: a WAF-blocked Cognito call is refused with a non-AWS message — ForbiddenException says "Request not allowed due to WAF rules." instead of "Request not allowed due to WAF block."

> _Note: this issue was identified against the code base at commit `afb41014a6bc1e75e8336e677aedbec1633a6643`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InitiateAuth

## Description
ONE cause.

Cause 1 of 1: when a REGIONAL web ACL associated with a Cognito user pool BLOCKs a call, the Cognito Identity Provider refusal carries the wrong message text.
AWS: Smithy (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice): ForbiddenException is an error structure, errorKind client, httpError 403, with a single member `message` documented as "The message returned when WAF doesn't allow your request based on a web ACL that's associated with your user pool"; the exception itself is documented as "thrown when WAF doesn't allow your request based on a web ACL that's associated with your user pool". Recorded behavior for a WAF Block decision on InitiateAuth: HTTP 403, error code ForbiddenException, and the message "Request not allowed due to WAF block." — the whole sentence is fixed text for this error class; it carries no values from the request or from state, so there is nothing per-run in it.
Emulator: HTTP 403 and code ForbiddenException are correct, but the message is "Request not allowed due to WAF block."'s near-miss "Request not allowed due to WAF rules." — the recorded failure is {'Message': 'Request not allowed due to WAF rules.', 'Code': 'ForbiddenException'}.
Why: for this error class the message is the only field carrying information, and AWS words it to distinguish a WAF Block from other WAF outcomes. Clients, log-scrapers and conformance tests that match on AWS's sentence (a common pattern because ForbiddenException has no other discriminating member) do not match, so a WAF block is reported as an unrecognized 403.
Fix: the message literal used when the WAF enforcement point in front of the Cognito Identity Provider operations serves a Block decision — i.e. the ForbiddenException construction in that enforcement/refusal path, not the WAF rule evaluation itself (status, code, and which call is blocked are already correct).
Traces: cognito-idp__wafv2.xsvc-cognito-idp-3-wafv2-block-rule-matching-header-blocked-unmatched-call-allowed.

Note for the fixer: the open issue 20261025T092843Z-cognito-idp-wafv2-a-regional-web-acl-associated.md reports that the association never took effect at all; enforcement now works (the marked call IS refused 403/ForbiddenException and the unmarked control is allowed), and this message-text divergence is what remains. The replay report marks this check neither ALREADY FILED nor OWNED.

## Evidence
Replay of simparity-behavioral-aws-v1 against the booted emulator: 163/209 passed (78.0%); cognito-idp__wafv2 0/1.

Failing check cognito-idp__wafv2.xsvc-cognito-idp-3-wafv2-block-rule-matching-header-blocked-unmatched-call-allowed, at the assertion `assert resp["Error"].get("Message") == "Request not allowed due to WAF block."`:
  AssertionError: {'Message': 'Request not allowed due to WAF rules.', 'Code': 'ForbiddenException'}

The two preceding assertions in the same check passed: the InitiateAuth carrying the marked header (x-sim2real-waf-marker) was refused with HTTPStatusCode 403 and Error.Code 'ForbiddenException' under a web ACL whose single rule is a header-match Block, so only the message text diverges.

Expected (AWS): HTTP 403, code ForbiddenException, message "Request not allowed due to WAF block." (entirely fixed text for this error class).
Actual (emulator): HTTP 403, code ForbiddenException, message "Request not allowed due to WAF rules."

Model reference: com.amazonaws.cognitoidentityprovider#ForbiddenException — errorKind client, httpError 403, single member `message` (MessageType), documented as the message returned when WAF doesn't allow the request based on the user pool's associated web ACL.

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay the behavioral suite (run_behavioral_suite) and observe the check id in trace_ids.
3. Create a Cognito user pool and app client, and a confirmed user.
4. CreateWebACL (Scope=REGIONAL) with one rule whose statement matches a single header (e.g. ByteMatchStatement on x-sim2real-waf-marker) and whose Action is Block, default action Allow; AssociateWebACL it to the user pool ARN.
5. Call InitiateAuth carrying that header and poll until it is refused -> the call is correctly refused with HTTP 403 / ForbiddenException, but Error.Message is 'Request not allowed due to WAF rules.' where AWS returns 'Request not allowed due to WAF block.'
6. Call InitiateAuth without the header to confirm the control still succeeds (200 with AuthenticationResult).
