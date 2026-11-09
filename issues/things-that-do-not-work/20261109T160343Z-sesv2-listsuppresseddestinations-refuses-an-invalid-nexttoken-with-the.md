Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2.list-suppressed-destinations-invalid-next-token-rejected
Issue-Shape-Id: com.amazonaws.sesv2#SimpleEmailService_v2
Issue-Key: sesv2.list-suppressed-destinations-invalid-next-token-rejected|run#3|AssertionError|assert err == ("InvalidNextTokenException", "Token is invalid."), (
Issue-Commit: 5b2a9eedb8fa287cac2318fd09ae07f52de03ab6

# sesv2: ListSuppressedDestinations refuses an invalid NextToken with the message 'Invalid next token.' where SES words it 'Token is invalid.'

> _Note: this issue was identified against the code base at commit `5b2a9eedb8fa287cac2318fd09ae07f52de03ab6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListSuppressedDestinations

## Description
Cause 1 of 1: ListSuppressedDestinations rejects a malformed/foreign pagination token with the right status and the right error code but the wrong message sentence. Failing check: sesv2.list-suppressed-destinations-invalid-next-token-rejected.
AWS: Smithy (com.amazonaws.sesv2): ListSuppressedDestinations GET /v2/email/suppression/addresses, input {TenantName, Reasons, StartDate, EndDate, NextToken (httpQuery 'NextToken'), PageSize (httpQuery 'PageSize')}, paginated {inputToken NextToken, outputToken NextToken, pageSize PageSize}; modeled errors BadRequestException(400), InvalidNextTokenException(400), NotFoundException(404), TooManyRequestsException(429). Recorded behavior for the un-decodable-token class on THIS operation: InvalidNextTokenException/400 with Message 'Token is invalid.' — the whole sentence is fixed text, the offending token is never echoed. The same call without a token still succeeds and returns SuppressedDestinationSummaries.
Emulator: the 400 and the InvalidNextTokenException code are right, but the message reads 'Invalid next token.' where this operation words it 'Token is invalid.' The positive control (the unpaginated call) succeeds, so only the refusal text diverges. Note the two sesv2 listings do NOT share one sentence: ListDedicatedIpPools words its forged-token BadRequestException 'Invalid next token.' (open issue 20261102T110414Z-sesv2-3-net-new-refusal-message-divergences-listdedicatedippools.md, Cause 1) while ListSuppressedDestinations' InvalidNextTokenException reads 'Token is invalid.'; a single shared sentence cannot satisfy both, and that open issue neither names this operation nor carries this check's replay key.
Why: a paging client that branches on, or a fixture that asserts, SES's documented cursor-rejection sentence treats a correctly refused token as an unknown failure, and code that distinguishes 'bad/expired cursor' from other 400s by message cannot do so.
Fix: the NextToken decode/validation refusal in the ListSuppressedDestinations handler of the sesv2 service (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/list_suppressed_destinations.rs) — and, if the sentence is authored in a shared cursor helper, that helper must let each listing emit its own operation's wording rather than one global sentence.
Traces: sesv2.list-suppressed-destinations-invalid-next-token-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator; verdict [mismatch] (the call was answered and refused; only the message text diverges).
Expected (AWS): ('InvalidNextTokenException', 'Token is invalid.').
Actual (emulator): AssertionError: expected InvalidNextTokenException, got ('InvalidNextTokenException', 'Invalid next token.')
The positive control — sesv2.list_suppressed_destinations() with no token returning SuppressedDestinationSummaries — is not reached, as the message assertion fails first.

## Steps to Reproduce
1. sesv2.list_suppressed_destinations(NextToken='not-a-real-pagination-token') — a token this service never issued.
2. Inspect Error.Code/Error.Message: the code InvalidNextTokenException and the 400 are right; the message is 'Invalid next token.' instead of 'Token is invalid.'
3. sesv2.list_suppressed_destinations() with no token still succeeds and returns SuppressedDestinationSummaries.
4. Re-replay sesv2.list-suppressed-destinations-invalid-next-token-rejected.
