Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e39df06442d4f2a86a0fa0e3a1f1c22c399555b8

# SSML that is not well-formed XML is accepted: character data outside the root element, and a second root element, pass as valid SSML (and are billed)

_Filed: 2026-10-28T15:58:22Z_
_Operation: SynthesizeSpeech | Class: validation_

> _Note: this issue was identified against the code base at commit `e39df06442d4f2a86a0fa0e3a1f1c22c399555b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`billed_characters` is the only place `TextType: ssml` input is validated, and its two structural checks are `!root_seen` (no element at all) and `depth != 0` (an element left unclosed). Neither of them notices the two other ways an SSML document can fail to be well-formed XML:

1. CHARACTER DATA OUTSIDE THE ROOT ELEMENT. The `Event::Text` arm bills every text event with no test of `depth` or `root_seen`, so `Text = "junk<speak>hi</speak>"` is read as a valid document: `root_seen` becomes true when `<speak>` opens, `depth` returns to 0, and the handler answers 200 — with the four characters that sit OUTSIDE the markup counted into `RequestCharacters` (6 instead of 2). XML 1.0 admits only whitespace, comments and processing instructions outside the root element, so such a document is not well-formed and `InvalidSsmlException` is the modelled answer.
2. A SECOND ROOT ELEMENT. `check_ssml_element` applies the document-element rule only `if !*root_seen`, and sets the flag on the FIRST element it sees. A later top-level element is then checked as an ordinary vocabulary element, so `"<speak>a</speak><speak>b</speak>"` — two root elements, again not well-formed XML — is accepted with 200.

The handler already intends to be the well-formedness gate (it hand-rolls the `root_seen`/`depth` checks precisely because quick_xml's pull reader validates no document structure), so this is an incomplete check rather than an unattempted feature: the same function that refuses `<speak>unclosed` and `<voice>hello</voice>` lets these through.

## Evidence

Smithy, `SynthesizeSpeech` operation documentation (smithy_query(query="doc", shape="SynthesizeSpeech")): "Synthesizes UTF-8 input, plain text or SSML, to a stream of bytes. SSML input must be valid, well-formed SSML." And `InvalidSsmlException`: "The SSML you provided is invalid. Verify the SSML syntax, spelling of tags and values, and then try again."

The crate's own claim, services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:686-689: "The\n/// SSML document is read with a real XML parser and every element it carries is\n/// checked against the published vocabulary ([`ssml`]), so `Err` here means the SSML\n/// is not well-formed, or not the SSML Amazon Polly accepts — both of which are\n/// `InvalidSsmlException`."

The code that does not keep it:
- synthesize_speech.rs:708-715 — `Event::Text(ref t) => { ... billed = billed.saturating_add(decoded.chars().count()); }` — billed with no `depth`/`root_seen` test, so text outside the root element is both accepted and billed.
- synthesize_speech.rs:737-742 — the ONLY structural checks: `if !root_seen { return Err("the text carries no SSML element") }` / `if depth != 0 { return Err("an SSML element is left unclosed") }`.
- synthesize_speech.rs:753-759 — `if !*root_seen { if name != "speak" { return Err(...) } *root_seen = true; }` — the document-element rule stops applying after the first element, so a second top-level `<speak>` is accepted.

Test coverage: tests/all/ssml_vocabulary.rs:190-216 covers the vocabulary and the single case "wrong document element" (`<voice>hello</voice>`); tests/all/lexicons_and_speech.rs:823-836 covers `"<speak>unclosed"`. No test in the crate presents character data outside the root element or a second root element, so neither case is pinned.

## Steps to Reproduce

1. POST /v1/speech with body {"Text": "junk<speak>hi</speak>", "TextType": "ssml", "VoiceId": "Joanna", "OutputFormat": "mp3"}
2. Observe HTTP 200 with `x-amzn-RequestCharacters: 6` — the four characters outside the markup were accepted and billed; expected HTTP 400 `InvalidSsmlException` because the document is not well-formed XML
3. POST /v1/speech with body {"Text": "<speak>a</speak><speak>b</speak>", "TextType": "ssml", "VoiceId": "Joanna", "OutputFormat": "mp3"}
4. Observe HTTP 200; expected HTTP 400 `InvalidSsmlException` — a document has exactly one root element
