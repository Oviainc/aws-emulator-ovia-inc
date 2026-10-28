Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# OutputFormat=json with TextType=ssml answers 501 "speech marks over ssml input" even when the request asks for no speech marks at all

_Filed: 2026-10-28T15:58:22Z_
_Operation: SynthesizeSpeech | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

In the `("json", _)` arm of the output dispatch, the SSML refusal is taken before the mark list is consulted and does not depend on it: `if text_type == "ssml" { return Err(not_implemented("SynthesizeSpeech speech marks over ssml input")) }`. A request that carries `OutputFormat: json`, `TextType: ssml` and NO `SpeechMarkTypes` therefore gets a 501 whose message names speech marks the request never asked for.

The branch's stated reason is that a mark's `start`/`end` are offsets into `Text` and marks over SSML would need offsets that skip the markup. That reason applies only when marks are actually computed. When `SpeechMarkTypes` is absent, `speech_marks` is called with both flags false and returns no marks, so nothing has to be offset — and the comment three lines below states the rule for exactly that request: naming no mark type asks for no marks, so the stream is empty rather than a refusal. The crate already implements and pins that answer for plain-text input; the unconditional guard turns the same request into a 501 purely because `TextType` is `ssml`.

## Evidence

The guard, services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:1296-1306: "// A mark's `start`/`end` are offsets into `Text` and its `value` is\n// the text it covers, so marks over SSML would have to be computed\n// from the document's text content with offsets that skip the markup.\n// That is not implemented ... so SSML input\n// is refused here rather than mis-answered.\nif text_type == \"ssml\" {\n    return Err(crate::scaffolded::Scaffolded::not_implemented(\n        \"SynthesizeSpeech speech marks over ssml input\",\n    ));\n}"

The rule it breaks, three lines later at synthesize_speech.rs:1307-1316: "let computed = speech_marks(&input.text, marks.iter().any(|mark| mark == \"sentence\"), marks.iter().any(|mark| mark == \"word\"));\n// A request that names no mark type asks for no marks, so the stream\n// is empty. `json` output IS the speech-mark stream, and Amazon Polly\n// declares no error for an empty one, so an empty stream is the\n// honest answer rather than a refusal it never sends.\nspeech_marks_stream(&computed)" — unreachable for `ssml` input.

Test coverage: tests/all/lexicons_and_speech.rs:902-916 pins the empty-stream answer ("Asking for no mark type asks for no marks, so the stream is empty — not a refusal Amazon Polly does not declare") for `TextType` text only, and tests/all/lexicons_and_speech.rs:921-941 pins the 501 only for NON-empty mark types (["word"], ["sentence"], ["sentence","word"]). The no-marks + ssml combination is pinned by nothing.

## Steps to Reproduce

1. POST /v1/speech with body {"Text": "<speak>hi</speak>", "TextType": "ssml", "VoiceId": "Joanna", "OutputFormat": "json"} and NO SpeechMarkTypes member
2. Observe HTTP 501 `NotImplemented` with the message naming "speech marks over ssml input"
3. Expected, per the handler's own rule at synthesize_speech.rs:1312-1316: HTTP 200, Content-Type application/x-json-stream, an empty body — the same answer the identical request with "TextType": "text" already gets (tests/all/lexicons_and_speech.rs:902-916)
