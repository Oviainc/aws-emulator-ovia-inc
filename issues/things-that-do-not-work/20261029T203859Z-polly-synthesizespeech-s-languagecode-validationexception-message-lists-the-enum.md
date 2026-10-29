Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.synthesize-speech-language-not-supported-error
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.synthesize-speech-language-not-supported-error|run#6|AssertionError|assert message == (
Issue-Commit: 6074294cbfccbc2257fa0c432afede02752270ab

# SynthesizeSpeech's LanguageCode ValidationException message lists the enum value set in model-declaration order and includes en-SG, not the set and order Polly reports

> _Note: this issue was identified against the code base at commit `6074294cbfccbc2257fa0c432afede02752270ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SynthesizeSpeech

## Description
Cause 1 of 1: the boundary enum-constraint message for SynthesizeSpeech$LanguageCode renders the wrong value set.

AWS: Smithy (services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json) declares SynthesizeSpeechInput$LanguageCode targeting the closed enum com.amazonaws.polly#LanguageCode, so a value outside the enum is a request-shape violation, not a business-logic refusal (the modeled LanguageNotSupportedException is reserved for a value that IS an enum member but is unsupported for the request). Recorded behavior: the refusal is HTTP 400 with Error.Code = "ValidationException" and Error.Message = `1 validation error detected: Value '<languageCode>' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [<comma-separated enum values>]`. Fixed text: everything outside the two substitutions. Variable parts: `<languageCode>` is the rejected value taken verbatim from the request, and `<comma-separated enum values>` is the service's own accepted-value set — a per-service constant, not a per-run value. The recorded set has 41 members, does NOT include `en-SG`, and is not in model-declaration order (it begins `en-IE, ar-AE, en-US, fr-BE, en-IN, ...`).

Emulator: the same status (400) and the same error code (ValidationException) are produced and the fixed text matches, but the value set is rendered straight from the model's declaration order and includes the extra member `en-SG`: `... Member must satisfy enum value set: [arb, cmn-CN, cy-GB, da-DK, de-DE, en-AU, en-GB, en-GB-WLS, en-IN, en-US, es-ES, es-MX, es-US, fr-CA, fr-FR, is-IS, it-IT, ja-JP, hi-IN, ko-KR, nb-NO, nl-NL, pl-PL, pt-BR, pt-PT, ro-RO, ru-RU, sv-SE, tr-TR, en-NZ, en-ZA, ca-ES, de-AT, yue-CN, ar-AE, fi-FI, en-IE, nl-BE, fr-BE, cs-CZ, de-CH, en-SG]` — 42 members, byte-identical to smithy_query(shape="LanguageCode").values in order. Hypothesis (neither the model nor the recording states this): the renderer iterates the generated enum's declaration order, while Polly serializes its own accepted set in a different, service-fixed order and does not list en-SG in this message.

Why: the enum value set is the actionable half of this message — it is what tells a caller, a CLI user or a log-scraping test which language codes are accepted. A client or test that compares the message, or parses the bracketed list to retry with a supported code, reads a set that differs from Polly's in both ordering and membership, and `en-SG` is advertised as accepted by the very error the service uses to enumerate what it accepts.

Fix: belongs in the request-shape validation layer that renders constraint violations for this operation's input (the enum-constraint violation message for SynthesizeSpeechInput$LanguageCode), not in the SynthesizeSpeech handler and not in the model: the status and code are already right, only the rendered value set is wrong. The list this message emits must be the service's accepted set in the order Polly reports it, independent of the order the enum happens to be declared in.

Traces: polly.synthesize-speech-language-not-supported-error (assertion at check line 33, on the message of the LanguageCode='xx-XX' refusal). The check's later legs — a valid-but-voice-mismatched LanguageCode accepted permissively, and the voice's own language succeeding — are not reached by this failure; the audio-payload fidelity they touch is covered by 20261022T175007Z-polly-synthesizespeech-returns-placeholder-audio-payloads-are-digital.md.

## Evidence
Expected (recorded AWS), for SynthesizeSpeech(Text=..., OutputFormat="mp3", VoiceId="Aditi", LanguageCode="xx-XX"): HTTP 400, Error.Code = "ValidationException", Error.Message = `1 validation error detected: Value 'xx-XX' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [en-IE, ar-AE, en-US, fr-BE, en-IN, es-MX, en-ZA, tr-TR, ru-RU, ro-RO, pt-PT, pl-PL, nl-NL, it-IT, is-IS, fr-FR, fi-FI, es-ES, de-DE, yue-CN, ko-KR, en-NZ, en-GB-WLS, hi-IN, de-CH, arb, nl-BE, cy-GB, cs-CZ, cmn-CN, da-DK, en-AU, pt-BR, nb-NO, sv-SE, ja-JP, es-US, ca-ES, fr-CA, en-GB, de-AT]` (41 members, no en-SG).
Actual (emulator): same status and code, message = `1 validation error detected: Value 'xx-XX' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [arb, cmn-CN, cy-GB, da-DK, de-DE, en-AU, en-GB, en-GB-WLS, en-IN, en-US, es-ES, es-MX, es-US, fr-CA, fr-FR, is-IS, it-IT, ja-JP, hi-IN, ko-KR, nb-NO, nl-NL, pl-PL, pt-BR, pt-PT, ro-RO, ru-RU, sv-SE, tr-TR, en-NZ, en-ZA, ca-ES, de-AT, yue-CN, ar-AE, fi-FI, en-IE, nl-BE, fr-BE, cs-CZ, de-CH, en-SG]` (42 members, en-SG present), raised as `AssertionError` at `assert message == (...)` in check_polly_synthesize_speech_language_not_supported_error.py:33.
Two differences, both confined to the bracketed set: (1) ordering — the emulator emits model-declaration order, byte-identical to smithy_query(shape="LanguageCode").values; (2) membership — the emulator adds `en-SG`. The fixed text, the quoted rejected value 'xx-XX' and the member path 'languageCode' already match.
Model: smithy_query(operation="SynthesizeSpeech") -> input member LanguageCode targets enum LanguageCode; modeled errors include LanguageNotSupportedException (400), a different layer that is not what the recording returns here.

## Steps to Reproduce
1. Boot the emulator and call POST /v1/speech with {"Text": "Testing unsupported language code validation.", "OutputFormat": "mp3", "VoiceId": "Aditi", "LanguageCode": "xx-XX"}.
2. Observe HTTP 400 ValidationException whose message ends `Member must satisfy enum value set: [arb, cmn-CN, ... , de-CH, en-SG]` — the model's declaration order, with en-SG.
3. Real Polly's message for the same request ends with a 41-member set in its own fixed order beginning `[en-IE, ar-AE, en-US, fr-BE, en-IN, ...]` and omitting en-SG.
4. Re-replay polly.synthesize-speech-language-not-supported-error to confirm the fix.
