Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0f7bb362ed1d3a4f050d8269ef5300e336140f8b

# SynthesizeSpeech accepts SSML that uses elements/attributes Polly does not support instead of answering InvalidSsmlException

_Filed: 2026-10-24T15:33:21Z_
_Operation: SynthesizeSpeech | Class: validation_

> _Note: this issue was identified against the code base at commit `0f7bb362ed1d3a4f050d8269ef5300e336140f8b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

With `TextType=ssml`, the handler validates the document only for (a) XML well-formedness, (b) a `<speak>` document element and (c) balanced tags. Anything that passes those three checks is synthesized with HTTP 200, whatever markup it contains.

Amazon Polly validates SSML against its own finite, documented tag vocabulary (`speak`, `break`, `emphasis`, `lang`, `mark`, `p`, `phoneme`, `prosody`, `s`, `say-as`, `sub`, `w`, `audio`, `amazon:effect`, `amazon:domain`, `amazon:breath`, …) and against the value domains of those tags' attributes, and refuses anything outside them with the modeled `InvalidSsmlException` (HTTP 400, message "Invalid SSML request").

Expected (real Polly):
* `<speak><voice name="Kevin">hello</voice></speak>` → 400 `InvalidSsmlException` (`voice` is not a Polly-supported element)
* `<speak><bogus>hello</bogus></speak>` → 400 `InvalidSsmlException`
* `<speak><prosody rate="supersonic">hello</prosody></speak>` → 400 `InvalidSsmlException` (attribute value outside the documented domain)
* `<speak><break time="30s"/>hello</speak>` → 400 `InvalidSsmlException` (`break time` is capped at 10s)

Actual (emulator): all four return HTTP 200 with an audio payload and an `x-amzn-RequestCharacters` count computed from the text content, because the only SSML refusals the handler can produce are "not well-formed" and "document element is not <speak>".

The effect is that a client validating its SSML templates against the emulator gets a green light for markup real Polly rejects — the inverse of the usual stub problem, and invisible until the same request hits AWS. This is a distinct class from the already-filed SynthesizeSpeech wrong-behavior issue (which is about SSML that *is* valid having no effect on the rendered audio) and from the already-filed error-response issue (voice/engine mismatch code).

## Evidence

Spec: `smithy_query(operation="SynthesizeSpeech")` declares `TextType` (`text|ssml`) and models `InvalidSsmlException` (HTTP 400) whose documentation is "The SSML you provided is invalid. Verify the SSML syntax, spelling of tags and values, and then try again." — "spelling of tags and values" is exactly the vocabulary/attribute-domain check, not mere well-formedness. Polly's supported-SSML-tag list is the published contract for what the service accepts.

Handler: services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs — `billed_characters()` (file lines ~324-364) runs `quick_xml::Reader` over the text and only maps a reader error, an absent root, or `depth != 0` to `Err`; `check_ssml_root()` (lines ~366-380) rejects a first element whose local name is not `speak` and then sets `root_seen`, after which *every* further element is accepted without inspection (`if *root_seen { return Ok(()) }`). No element-name allow-list and no attribute inspection exists anywhere in the crate. In `handle` (lines ~797-801) the only `InvalidSsmlException` is the `map_err` on that function.

Test coverage: tests/all/lexicons_and_speech.rs line 763 (`"<speak>unclosed"`), tests/all/error_semantics.rs line 433 (`["<speak>unclosed", "not ssml at all <", "<speak>a</speak"]`) and tests/all/error_document_message.rs line 314 exercise InvalidSsmlException only for malformed XML; tests/all/lexicons_and_speech.rs lines 715-718 assert that `<speak>one <break time="1s"/>two</speak>` is accepted and billed. Grepping the tests for `speak|InvalidSsml` returns no case with an unsupported element or an out-of-domain attribute value, so no test asserts either behaviour and the gap is uncovered.

## Steps to Reproduce

1. Boot the emulator and point a botocore `polly` client at it
2. Call `polly.synthesize_speech(Text='<speak><voice name="Kevin">hello</voice></speak>', TextType='ssml', OutputFormat='mp3', VoiceId='Joanna')`
3. Observe HTTP 200 with an `audio/mpeg` payload; real Polly answers HTTP 400 `InvalidSsmlException` ("Invalid SSML request") because `<voice>` is not a supported SSML element
4. Repeat with `Text='<speak><bogus>hello</bogus></speak>'`, `Text='<speak><prosody rate="supersonic">hello</prosody></speak>'` and `Text='<speak><break time="30s"/>hello</speak>'` — each is accepted with 200 where real Polly refuses with `InvalidSsmlException`
5. Confirm the emulator's only SSML refusals are syntactic: `Text='<speak>unclosed'` and `Text='<notspeak/>'` do answer 400 `InvalidSsmlException`
