Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm.list-certificates-pagination-next-token-exactness
Issue-Shape-Id: com.amazonaws.acm#CertificateManager
Issue-Key: acm|ListCertificates|InvalidArgsException
Issue-Commit: 595d3d5604be8cb7f050ded6602d61000805e4f4

# ACM ListCertificates rejects a malformed NextToken with the wrong InvalidArgsException message (1 cause)

> _Note: this issue was identified against the code base at commit `595d3d5604be8cb7f050ded6602d61000805e4f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListCertificates

## Description
Cause 1 of 1: the InvalidArgsException raised for an unusable ListCertificates NextToken carries a message of the emulator's own invention instead of ACM's.

AWS: Smithy ListCertificates declares errors InvalidArgsException (client, HTTP 400) and ValidationException, and ListCertificatesRequest.NextToken is a string (@length 1..10000, pattern ^[\u0009\u000A\u000D\u0020-\u00FF]*$) which the service mints and must be able to decode. A syntactically admissible but un-issued cursor therefore passes input validation and is refused by the operation itself. Recorded behavior (the check states the expectation is confirmed against real AWS): HTTP 400, Error.Code 'InvalidArgsException', Error.Message 'Invalid Next Token received <the NextToken sent>'. Message rule: the fixed text is 'Invalid Next Token received ' and the one variable part is the NextToken value copied verbatim from the request — no period, no quoting. The token in the failing check ('sim2real-not-a-valid-next-token') is only the value that filled that template on this run, not part of the requirement.

Emulator: the status (400) and the code (InvalidArgsException) are right, but the message is the fixed string 'NextToken is not a token this service issued' — different wording and, crucially, it does not echo the rejected cursor at all.

Why: a client or test that matches ACM's refusal text (the common pattern for telling 'my cursor is stale/forged' apart from other InvalidArgsException causes such as a bad filter combination) never matches, so stale-cursor handling that works against AWS silently falls through to a generic error path. Operators also lose the only diagnostic AWS gives them — which cursor value was refused — so a paginator bug that corrupts the token cannot be identified from the error.

Fix: the NextToken decode/refusal path of the ListCertificates handler in the ACM service crate (the pagination-cursor validation that constructs InvalidArgsException), not the shared constraint-violation builder: the ValidationException envelope covered by open issue 20261022T143245Z-acm-certificate-representation-dns-validation-and-error-message.md (Cause 7) is a different error class and that issue does not carry this check. Build the message from the request's NextToken as AWS words it.

Traces: acm.list-certificates-pagination-next-token-exactness (the preceding assertions in the same check — stable tokens, no duplicate or missing rows across MaxItems=1 and MaxItems=100 walks, and the refusal's code — all passed; only the message diverged).

## Evidence
acm.list-certificates-pagination-next-token-exactness, check_acm_list_certificates_pagination_next_token_exactness.py line 232:
AssertionError: expected message 'Invalid Next Token received sim2real-not-a-valid-next-token' (confirmed against real AWS), got 'NextToken is not a token this service issued'

Expected (AWS): HTTP 400, Error.Code='InvalidArgsException', Error.Message='Invalid Next Token received ' + <NextToken from the request>.
Actual (emulator): HTTP 400, Error.Code='InvalidArgsException' (correct), Error.Message='NextToken is not a token this service issued' (fixed text, request value dropped).

Smithy citation: ListCertificates errors = [InvalidArgsException (errorKind client, httpError 400), ValidationException]; ListCertificatesRequest.NextToken -> NextToken @length(min 1, max 10000) @pattern ^[\u0009\u000A\u000D\u0020-\u00FF]*$; paginated(inputToken=NextToken, outputToken=NextToken, items=CertificateSummaryList, pageSize=MaxItems).

## Steps to Reproduce
1. Boot the emulator and point an ACM client at it.
2. Request a few certificates so ListCertificates has rows (not required for the refusal, but matches the check).
3. Call acm.list_certificates(MaxItems=1, NextToken='sim2real-not-a-valid-next-token') — any string that satisfies the modeled NextToken pattern but was never issued by the service.
4. Observe the ClientError: Error.Code is 'InvalidArgsException' (correct) but Error.Message is 'NextToken is not a token this service issued' instead of 'Invalid Next Token received sim2real-not-a-valid-next-token'.
5. Repeat with a different bogus token value and confirm the emulator's message never varies, i.e. the request's NextToken is not interpolated.
6. Re-replay acm.list-certificates-pagination-next-token-exactness.
