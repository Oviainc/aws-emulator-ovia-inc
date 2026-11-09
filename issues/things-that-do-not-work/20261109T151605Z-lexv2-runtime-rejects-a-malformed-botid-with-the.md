Issue-Source: trace-sweep-divergence
Issue-Trace: lex-runtime-v2-runtime-rejects-malformed-and-unknown-identifiers
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lex-runtime-v2-runtime-rejects-malformed-and-unknown-identifiers
Issue-Shape-Id: com.amazonaws.lexruntimev2#AWSDeepSenseRunTimeServiceApi2_0
Issue-Key: lexv2-runtime|GetSession|4xx->4xx|$.message
Issue-Key: lexv2-runtime|PutSession|4xx->4xx|$.message
Issue-Commit: 3d0a54f64a8eacec16ca43902aa47fe37888ae5e

# lexv2-runtime rejects a malformed botId with the framework's generic constraint message, not the Lex runtime's "INVALID_REQUEST - Invalid bot name or alias"

> _Note: this issue was identified against the code base at commit `3d0a54f64a8eacec16ca43902aa47fe37888ae5e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSession

## Description
Two net-new causes in lexv2-runtime, one root fault: the identifier-validation path that guards the four @httpLabel values of a session address answers with the emulator framework's GENERIC Smithy constraint report instead of the fixed message the Amazon Lex V2 runtime words for a bad bot identifier. Both causes are the same wording defect reached through two different operations, so a fixer must close BOTH (the same rejection is reached from RecognizeText, RecognizeUtterance and DeleteSession as well).

No open issue covers this: the existing open lexv2-runtime issue (20261107T161252Z-lexv2-runtime-recognizetext-reports-intent-slots-holding-only-filled-slots.md) is about Intent.slots contents on a successful turn, not about identifier rejection wording.

Cause 1 of 2: GetSession with a botId that violates the modelled identifier shape returns the wrong ValidationException message.
AWS: Smithy com.amazonaws.lexruntimev2#GetSession binds botId as @httpLabel @required targeting BotIdentifier, which is @length(min: 10, max: 10) @pattern("^[0-9a-zA-Z]+$"); the operation models ValidationException (HTTP 400). The recorded response for a botId that is not a well-formed bot identifier is HTTP 400 with the message "INVALID_REQUEST - Invalid bot name or alias" — that whole string is FIXED text: the Lex V2 runtime frontend interpolates NOTHING from the request into it (no member name, no echoed value, no limit). The status (400) and the error code are taken from the recorded response and both already match; only the message diverges.
Emulator: HTTP 400 with the framework's generic constraint report, "1 validation error detected: Value 'AAAAAAAAAAA' at 'botId' failed to satisfy constraint: Member must have length less than or equal to 10". Besides being the wrong message class, it echoes the caller's value and the member name (neither of which AWS reveals here) and it states a rule the model does not have — BotIdentifier is exactly 10 characters (min 10 AND max 10), so "less than or equal to 10" tells a client that a 4-character botId would be accepted.
Why: a client that branches on the error message — a retry/repair path, a test asserting the service's rejection, or a UI that surfaces Lex's wording — sees an entirely different error text than AWS produces, so the "invalid bot name or alias" condition is unrecognisable, and the leaked "<= 10" rule misleads anyone deriving the identifier format from the emulator.
Fix: the identifier validation that precedes the handler in the lexv2-runtime crate (the @httpLabel constraint rejection shared by the session operations) must emit the Lex V2 runtime's fixed ValidationException message for a bot identifier that fails BotIdentifier's length or pattern, instead of letting the framework's generic constraint reporter serialize the violation.
Traces: lex-runtime-v2-runtime-rejects-malformed-and-unknown-identifiers#task#1 (GetSession).

Cause 2 of 2: PutSession with the same malformed botId returns the same wrong message.
AWS: Smithy com.amazonaws.lexruntimev2#PutSession binds the identical botId @httpLabel/BotIdentifier shape and models ValidationException (HTTP 400); the recorded response is HTTP 400 with the same fixed text "INVALID_REQUEST - Invalid bot name or alias".
Emulator: HTTP 400 with the same generic "1 validation error detected: Value '<botId>' at 'botId' failed to satisfy constraint: Member must have length less than or equal to 10" report.
Why: same as Cause 1 — and because both operations diverge identically, a client cannot discover the real wording from any session operation.
Fix: same component as Cause 1; the fix belongs in the shared validation path, not duplicated per operation, so that every operation addressing a session by botId answers with the one message.
Traces: lex-runtime-v2-runtime-rejects-malformed-and-unknown-identifiers#task#8 (PutSession).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (score 85.7%, 7 mismatched steps of 49).

Cause 1 — fixture lex-runtime-v2-runtime-rejects-malformed-and-unknown-identifiers, step task#1, operation GetSession, verdict mismatch, expected status 400 / actual status 400:
  $.message: value_mismatch (expected 'INVALID_REQUEST - Invalid bot name or alias', got "1 validation error detected: Value 'AAAAAAAAAAA' at 'botId' failed to satisfy constraint: Member must have length less than or equal to 10")

Cause 2 — same fixture, step task#8, operation PutSession, verdict mismatch, expected status 400 / actual status 400:
  $.message: value_mismatch (expected 'INVALID_REQUEST - Invalid bot name or alias', got "1 validation error detected: Value 'AAAAAAAAAAA' at 'botId' failed to satisfy constraint: Member must have length less than or equal to 10")

Fixed vs variable parts of the required message: "INVALID_REQUEST - Invalid bot name or alias" is entirely fixed — no part of it is taken from the request or from state. The emulator's message, by contrast, interpolates the request's botId value and the member name 'botId'.

Smithy cited: com.amazonaws.lexruntimev2#GetSessionRequest$botId and #PutSessionRequest$botId -> BotIdentifier, @httpLabel @required, length {min: 10, max: 10}, pattern ^[0-9a-zA-Z]+$; both operations model ValidationException with httpError 400.

## Steps to Reproduce
1. Boot the emulator from this checkout and point a Lex V2 runtime client at it.
2. Call GetSession with botId = an identifier that violates BotIdentifier (e.g. 11 alphanumeric characters), any botAliasId, localeId=en_US, a well-formed sessionId.
3. Observe HTTP 400. Emulator message: "1 validation error detected: Value '<the botId>' at 'botId' failed to satisfy constraint: Member must have length less than or equal to 10". AWS message: "INVALID_REQUEST - Invalid bot name or alias" (fixed text, nothing interpolated).
4. Call PutSession on the same malformed botId with any valid sessionState body. Same divergence: generic constraint report instead of the fixed Lex runtime message.
