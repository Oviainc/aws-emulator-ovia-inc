Issue-Source: trace-sweep-divergence
Issue-Trace: comprehend-detect-limit-errors
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: comprehend-detect-limit-errors comprehend-endpoint-management-errors
Issue-Shape-Id: com.amazonaws.comprehend#Comprehend_20171127
Issue-Key: comprehend|DetectPiiEntities|4xx->2xx|$.__type
Issue-Key: comprehend|DetectPiiEntities|4xx->2xx|$.message
Issue-Key: comprehend|DetectPiiEntities|4xx->2xx|status
Issue-Key: comprehend|ContainsPiiEntities|4xx->4xx|$.__type
Issue-Key: comprehend|ContainsPiiEntities|4xx->4xx|$.message
Issue-Key: comprehend|CreateEndpoint|4xx->4xx|$.message
Issue-Commit: 7b78f885090352093daa8ac758bd09ec0b97c628

# Comprehend input validation diverges: languageCode enum refusals use the wrong class or none, and the length message echoes the value

> _Note: this issue was identified against the code base at commit `7b78f885090352093daa8ac758bd09ec0b97c628`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DetectPiiEntities

## Description
Three net-new root causes in how Comprehend admits a synchronous-inference or control-plane request. The PII operations do not support every LanguageCode the shared enum declares, and AWS enforces that per-operation restriction at the request-validation boundary — as a frontend ValidationException naming the operation's own accepted value set — not as the modeled UnsupportedLanguageException. The emulator either picks the wrong class or does not enforce the restriction at all. A third cause is the boundary message's shape for a length violation.

Cause 1 of 3: DetectPiiEntities with an unsupported LanguageCode is ACCEPTED and answered 200.
  AWS: Smithy: DetectPiiEntities input LanguageCode @required, target LanguageCode (enum en, es, fr, de, it, pt, ar, hi, ja, ko, zh, zh-TW) — the shared enum is wider than this operation's support, and the recording shows the operation accepting only en and es. Recorded behavior: HTTP 400, __type 'ValidationException', message "Value '<languageCode>' at 'languageCode'failed to satisfy constraint: Member must satisfy enum value set: [en, es]". Fixed text: "Value '", "' at 'languageCode'failed to satisfy constraint: Member must satisfy enum value set: [", "]" (note AWS emits no space between the quoted value and "failed", and no leading "1 validation error detected:" for this clause); variable parts: the rejected languageCode from the request, and the accepted set, which is this operation's set.
  Emulator: HTTP 200 with a normal DetectPiiEntities result — the per-operation language restriction is not enforced anywhere.
  Why: a client is told an unsupported language succeeded and consumes PII findings produced under a language the real service refuses; the error path it has code for never runs.
  Fix: DetectPiiEntities' request validation (the per-operation accepted-language set), and the boundary ValidationException serializer.
  Traces: comprehend-detect-limit-errors#task#4.

Cause 2 of 3: ContainsPiiEntities with an unsupported LanguageCode is refused as UnsupportedLanguageException with no message member.
  AWS: Smithy: ContainsPiiEntities input LanguageCode @required, target LanguageCode; errors include UnsupportedLanguageException(400) and InvalidRequestException(400). Recorded behavior: the refusal is the boundary clause, HTTP 400, __type 'ValidationException', message "Value '<languageCode>' at 'languageCode'failed to satisfy constraint: Member must satisfy enum value set: [en]" — ContainsPiiEntities accepts en only. Same fixed text/variable split as cause 1.
  Emulator: HTTP 400 but __type 'UnsupportedLanguageException' and NO message member at all (the body carries no 'message').
  Why: the client catches a different modeled exception than AWS throws, and gets no text to show a user or to assert on; the accepted value set — the one piece of actionable information in AWS's clause — is absent.
  Fix: ContainsPiiEntities' request validation (raise the boundary enum clause rather than UnsupportedLanguageException) and its error serializer, which must always populate 'message'.
  Traces: comprehend-detect-limit-errors#task#3.

Cause 3 of 3: CreateEndpoint's length-constraint message interpolates the rejected EndpointName; AWS omits it for this member.
  AWS: Smithy: CreateEndpoint input EndpointName @required, target ComprehendEndpointName @length(max 40), @pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$. Recorded behavior: HTTP 400 with message "1 validation error detected: Value at 'endpointName' failed to satisfy constraint: Member must have length less than or equal to 40" — the whole clause is fixed text apart from the count, the member name 'endpointName' and the limit 40 taken from the model; the rejected VALUE is deliberately not echoed for this member.
  Emulator: the same clause with "Value '<endpointName>' at 'endpointName' ...", i.e. it splices the request's endpoint name into a position AWS leaves empty.
  Why: a client asserting on the documented clause never matches, and a name that may carry caller-chosen content is reflected back in an error body AWS keeps value-free.
  Fix: the shared constraint-validation message builder, for the no-echo case on this member.
  Traces: comprehend-endpoint-management-errors#task#1.

## Evidence
Cause 1 (DetectPiiEntities, comprehend-detect-limit-errors#task#4): status: expected 400, got 200; $.__type expected 'ValidationException', got None; $.message expected "Value 'fr' at 'languageCode'failed to satisfy constraint: Member must satisfy enum value set: [en, es]", got None. ('fr' is the fixture's rejected value filling the template's variable slot.)
Cause 2 (ContainsPiiEntities, comprehend-detect-limit-errors#task#3): $.__type expected 'ValidationException', got 'UnsupportedLanguageException'; $.message expected "Value 'fr' at 'languageCode'failed to satisfy constraint: Member must satisfy enum value set: [en]", got None.
Cause 3 (CreateEndpoint, comprehend-endpoint-management-errors#task#1): $.message expected "1 validation error detected: Value at 'endpointName' failed to satisfy constraint: Member must have length less than or equal to 40", got "1 validation error detected: Value 'sim2real-probe-comprehend-endpoint-27e1bfe8' at 'endpointName' failed to satisfy constraint: Member must have length less than or equal to 40" — the echoed string is the fixture's own request value; the defect is that any value is echoed.

## Steps to Reproduce
1. Boot the emulator from mainline and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture comprehend-detect-limit-errors, step task#4 — DetectPiiEntities with a LanguageCode outside {en, es}; the emulator answers 200 instead of the 400 ValidationException.
3. Cause 2: fixture comprehend-detect-limit-errors, step task#3 — ContainsPiiEntities with a LanguageCode other than en; the emulator answers UnsupportedLanguageException with no message.
4. Cause 3: fixture comprehend-endpoint-management-errors, step task#1 — CreateEndpoint with an EndpointName longer than 40 characters; the emulator's clause contains the rejected value.
