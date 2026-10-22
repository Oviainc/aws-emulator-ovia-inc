Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: uxc.list-services-malformed-next-token-rejected
Issue-Shape-Id: com.amazonaws.uxc#AWSAccountUXSetting
Issue-Key: uxc.list-services-malformed-next-token-rejected|run#8|AssertionError|assert message == _BOGUS_TOKEN_MESSAGE, (
Issue-Commit: 6d3a954d57304c873e4e6a3ccc977c1ac1664bbd

# ListServices: bogus (well-formed) nextToken rejection does not use the pagination-token exception message

> _Note: this issue was identified against the code base at commit `6d3a954d57304c873e4e6a3ccc977c1ac1664bbd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListServices

## Description
ONE cause, on uxc ListServices (GET /v1/services).

Related open issue: '20261022T030944Z-uxc-listservices-returns-wrong-service-catalog-and-validation-message.md'. Its Cause 6 concerns a nextToken that violates the member @pattern and how the anchored regex is rendered inside the generated 'N validation error detected' message. This cause is a DIFFERENT input class — a token that SATISFIES @length(32,512) and @pattern but is not a real continuation cursor — and the failing behavioral check here was not marked OWNED by that issue.

Cause 1 of 1: a well-formed but non-cursor nextToken is refused with the wrong message.
AWS: Smithy: ListServices.nextToken (httpQuery nextToken) @length(32,512) @pattern ^[-A-Za-z0-9+/_]+=*$; paginated inputToken=nextToken/outputToken=nextToken/items=services; modeled error ValidationException (httpError 400). For a token that passes those client-side constraints but does not decode as a cursor the recorded behavior is HTTP 400 with error code ValidationException and the pagination-framework message "Pagination token exception in operation 'ListServices': invalid payload wrapping schema 0" — a fixed template parameterized by the operation name, not free prose, so the template is the requirement (the service-independent part is constant and 'ListServices' comes from the request).
Emulator: HTTP 400 with error code ValidationException — status and code agree — but the message is the boundary-validator text 'nextToken: Member must be a nextToken returned by a previous ListServices response'. The check's positive control (a real nextToken from a prior maxResults=1 page advancing to a distinct second page) is not reached.
Why: a client or harness that distinguishes a stale/corrupt pagination cursor from an ordinary member-constraint violation does so by the message, since both classes share status 400 and the code ValidationException; against the emulator a cursor-decode failure is indistinguishable from a shape violation, and any assertion or log-matching on the pagination-token exception text diverges.
Fix: the ListServices handler's nextToken decode path (services/service-com-amazonaws-uxc-awsaccountuxsetting/src/ops/list_services.rs), which must render a cursor-decode failure with the pagination-token exception template rather than reusing the member-constraint message formatter; status 400 and code ValidationException are already correct.
Traces: uxc.list-services-malformed-next-token-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1 against this checkout: uxc 12/19 passed, 7 failed.

[mismatch] uxc.list-services-malformed-next-token-rejected — GET /v1/services?nextToken='A'*32 (32 chars, charset-valid, so it passes botocore and the modeled @length/@pattern): error code ValidationException matches, message diverges. AssertionError: expected message "Pagination token exception in operation 'ListServices': invalid payload wrapping schema 0" for a bogus nextToken, got 'nextToken: Member must be a nextToken returned by a previous ListServices response'.

## Steps to Reproduce
1. Boot the emulator from this checkout and run the behavioral suite simparity-behavioral-aws-v1 (service uxc).
2. Replay uxc.list-services-malformed-next-token-rejected: call ListServices(maxResults=1) to obtain a real nextToken, then call ListServices(nextToken='A'*32).
3. Read the error: status 400 and code ValidationException are correct; compare $.message against the pagination-token exception template "Pagination token exception in operation 'ListServices': invalid payload wrapping schema 0" — the emulator answers the member-constraint text instead.
4. Confirm the positive control still holds: ListServices(maxResults=1, nextToken=<real token>) must return a page whose first service differs from page one's.
