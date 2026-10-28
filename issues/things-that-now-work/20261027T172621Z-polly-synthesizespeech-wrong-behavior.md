Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: c2bd83202fa2a42770413e9befb5df83c0ed8d0c

# SynthesizeSpeech bills no characters for SSML text spelled with entity or character references

_Filed: 2026-10-27T17:26:21Z_
_Operation: SynthesizeSpeech | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `c2bd83202fa2a42770413e9befb5df83c0ed8d0c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`billed_characters` counts an SSML document's text content by walking quick-xml events, but its match arms only handle `Event::Text` and `Event::CData` — `Event::GeneralRef` falls into the catch-all `_ => {}`. With quick-xml 0.41 (the version this crate pins) the reader BREAKS text at a reference and emits the reference as its own event, which this crate's sibling PLS reader in `put_lexicon.rs` states explicitly and handles. The consequence is that every character an SSML `Text` spells with an entity reference (`&amp;`) or a numeric character reference (`&#120;`) is billed as ZERO characters: the `x-amzn-RequestCharacters` response header under-reports the characters synthesized, and the emulator's own derived values (`spoken_duration_ms(billed)`, hence the length of the returned stream) are computed from the same under-count. The `Event::Text` arm's comment claims the opposite rule ("Entities count as the character they stand for") and its `unescape` call can never fire for a reference, because the reference never arrives inside a `Text` event. This is a separate defect from the open "SynthesizeSpeech returns placeholder audio" issue, which is about the waveform, lexicon application, SSML rendering, ogg_vorbis and non-sentence/word marks — none of its five causes mention the billed-character computation.

## Evidence

Model, `SynthesizeSpeechOutput$RequestCharacters`: "Number of characters synthesized." Model, `TextLengthExceededException`: "the limit for input text is a maximum of 6000 characters total, of which no more than 3000 can be billed characters. ... SSML tags are not counted as billed characters." — so SSML TEXT CONTENT is billed, and a character written as `&amp;` is text content, not a tag.

The defect, services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:704-717:
```
            Event::Text(ref t) => {
                // Entities count as the character they stand for, so the raw bytes
                // are unescaped before counting.
                let raw = String::from_utf8_lossy(t.as_ref()).to_string();
                let decoded =
                    quick_xml::escape::unescape(&raw).map_err(|e| e.to_string())?;
                billed = billed.saturating_add(decoded.chars().count());
            }
            Event::CData(ref c) => { ... }
            _ => {}
```
There is no `Event::GeneralRef` arm, so a reference is counted as nothing.

The sibling reader in the SAME crate, services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs:459-473, states the rule and implements it:
```
            // The reader BREAKS text at a reference and emits the reference on its own,
            // so a loop reading only text events would measure none of the characters a
            // replacement spells with `&amp;` or `&#NN;`.
            Event::GeneralRef(ref reference) => {
                if let Some(open) = replacements.last_mut() {
                    let name = reference.decode()...;
                    let reconstructed = format!("&{name};");
                    let resolved = quick_xml::escape::unescape(&reconstructed)...;
                    *open = open.saturating_add(resolved.chars().count());
```
That sibling rule is pinned end-to-end by services/service-com-amazonaws-polly-parrot-v1/tests/all/pls_replacement_measurement.rs:153-179 ("An entity reference stands for the one character it names, so five of them are five characters of replacement and not thirty", plus the `"&#120;".repeat(quota + 1)` row), and the quick-xml version is services/service-com-amazonaws-polly-parrot-v1/Cargo.toml:29 `quick-xml = "0.41"` (Cargo.lock:4519-4520 `quick-xml 0.41.0`).

Test coverage of the billed rule exists but has no reference row: services/service-com-amazonaws-polly-parrot-v1/tests/all/lexicons_and_speech.rs:768-785 — "RULE: `RequestCharacters` is the billed character count — every character of plain text, and for SSML the text content only" — its table is only `"<speak>hello world</speak>"` and `"<speak>one <break time=\"1s\"/>two</speak>"`; and the unit test services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:1446-1456 `ssml_tags_are_not_billed` likewise uses no entity.

## Steps to Reproduce

1. POST /v1/speech with body {"Text": "<speak>a&amp;b</speak>", "TextType": "ssml", "VoiceId": "Joanna", "OutputFormat": "mp3"}
2. Read the `x-amzn-RequestCharacters` response header: it is 2 (only "a" and "b" were counted), while the billed text content is the 3 characters `a`, `&`, `b`
3. POST /v1/speech with body {"Text": "<speak>&#120;&#120;&#120;&#120;&#120;</speak>", "TextType": "ssml", "VoiceId": "Joanna", "OutputFormat": "mp3"}
4. `x-amzn-RequestCharacters` is 0 instead of 5, and because `spoken_duration_ms(billed)` is fed the same 0, the returned stream is the 20 ms floor (`audio::MIN_DURATION_MS`) rather than the duration the five characters imply
5. Contrast with PutLexicon, which measures the same spelling correctly: a `<alias>` of `"&amp;".repeat(101)` is refused as 101 characters (tests/all/pls_replacement_measurement.rs)
