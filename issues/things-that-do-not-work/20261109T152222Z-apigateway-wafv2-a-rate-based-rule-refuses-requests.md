Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers
Issue-Key: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers|run#20|AssertionError|assert first_refusal >= RATE_LIMIT, (
Issue-Commit: 39b863d101414613449d16e44f0b23fd775a8d87

# apigateway__wafv2: a rate-based rule refuses requests below its configured Limit

> _Note: this issue was identified against the code base at commit `39b863d101414613449d16e44f0b23fd775a8d87`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateWebACL

## Description
Cause 1 of 1: a WAFv2 rate-based rule blocks the caller before the configured request count is reached, so the refusal cannot be attributed to the rate limit.
AWS: a RateBasedStatement with a Limit of N and an aggregate key (e.g. IP) counts a caller's requests in the evaluation window and only begins applying the rule's action once the count EXCEEDS Limit; requests at or below Limit are not refused by that rule. After the window elapses the caller recovers and is served again, and the blocked key is reported by GetRateBasedStatementManagedKeys.
Emulator: with Limit=10, the 8th request from the caller is already refused with HTTP 403 — a refusal at or below the configured Limit, which therefore cannot be the rate-based rule's doing. The observed sequence was [200, 200, 200, 200, 200, 200, 200, 403, 403, 403, 403, 403].
Why: a caller is throttled earlier than the policy it wrote says, so a rate limit cannot be tuned or trusted; and because the refusal starts below Limit, a client cannot tell a rate-based block from some other 403 (the "lists its key then recovers" half of the contract is unverifiable).
Fix: the wafv2 rate-based rule evaluation's request counting and threshold comparison (apply the action only once the count exceeds Limit within the evaluation window), and the managed-keys listing/recovery that follows from it.
Traces: apigateway__wafv2.wafv2-rate-based-rule-refuses-the-caller-lists-its-key-then-recovers

## Evidence
"AssertionError: request #8 was refused, which is at or below the configured Limit of 10, so the refusal cannot be rate-based: [200, 200, 200, 200, 200, 200, 200, 403, 403, 403, 403, 403]". Expected: requests 1..Limit served, refusals only above Limit, the caller's key listed by GetRateBasedStatementManagedKeys, then recovery after the window. The Limit of 10 and the request count are the request's values; the requirement is the >Limit threshold.

## Steps to Reproduce
1. wafv2 CreateWebACL with a rule whose Statement is a RateBasedStatement (AggregateKeyType IP, Limit 10) and Action Block; associate it with a deployed API Gateway stage.
2. Issue 12 sequential requests from one source to the stage.
Expected: the first 10 are served and only requests above the Limit are refused 403; GetRateBasedStatementManagedKeys lists the key; after the window the caller is served again. Actual: request #8 is already refused 403.
