Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: f3979e0246435c1672d170ef00d4539e5318fdf8

# A PLS document carrying an unresolvable entity or character reference outside a replacement element is stored as a valid lexicon, although the sibling SSML reader refuses the same reference

_Filed: 2026-10-28T21:45:30Z_
_Operation: PutLexicon | Class: validation_

> _Note: this issue was identified against the code base at commit `f3979e0246435c1672d170ef00d4539e5318fdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`parse_pls` resolves a general/character reference ONLY when the read is currently inside an open `<alias>`/`<phoneme>` of a `<lexeme>`: the whole `Event::GeneralRef` body sits inside `if let Some(open) = replacements.last_mut()`. A reference anywhere else inside the document element — in a `<grapheme>`, in the text of a `<lexeme>`, or directly under `<lexicon>` — therefore falls through the `if let` and is never resolved and never checked, so a reference that stands for nothing (`&nosuchentity;`, `&#;`, `&#xFFFFFF;`) is silently ignored and the document is accepted, stored and reported by `GetLexicon`/`ListLexicons` as a valid PLS lexicon.

XML 1.0 admits no undeclared general entity reference, so such a `Content` is not one well-formed XML document — which is exactly the rule `parse_pls`'s own doc comment states it enforces ("any malformed one is refused"), and exactly the rule the crate's OTHER XML reader does enforce: `billed_characters` in `synthesize_speech.rs` runs the same decode/`unescape` pair UNGUARDED, so an unresolvable reference anywhere in an SSML document is `InvalidSsmlException` (pinned by an existing end-to-end test). The same byte sequence is therefore a refusal on the SSML door and a 200 on the lexicon door.

The guard is also inconsistent *within* `parse_pls`: the very same reference IS refused when it sits outside the document element (the `open_elements == 0` arm), and IS refused when it sits inside a replacement element (where the `if let` succeeds and the `?` propagates). Only the middle case — inside the document, outside a replacement — escapes the check.

The same `if let Some(open) = replacements.last_mut()` guard wraps the `unescape` call in the `Event::Text` arm, so any unescaping failure on text outside a replacement is likewise skipped; both are symptoms of one missing whole-document reference check.

Consequence: `StoredLexicon.content` holds a document that is not well-formed XML, `size` counts its stray characters, and `GetLexicon` hands the blob back verbatim as `Lexicon$Content`.

## Evidence

The rule `parse_pls` states for itself — services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs:349-351:
```
/// The reader is a real, NAMESPACE-AWARE XML parser, so any well-formed PLS document
/// is accepted whatever prefix it spells its elements with, and any malformed one is
/// refused — the shape of the examples this emulator has seen does not enter into it.
```

The reference check is reached only inside an open replacement — put_lexicon.rs:505-521:
```
            // The reader BREAKS text at a reference and emits the reference on its own,
            // so a loop reading only text events would measure none of the characters a
            // replacement spells with `&amp;` or `&#NN;`.
            Event::GeneralRef(ref reference) => {
                if let Some(open) = replacements.last_mut() {
                    let name = reference
                        .decode()
                        .map_err(|e| PlsRejection::NotALexicon(e.to_string()))?;
                    // Reconstructed and unescaped so a predefined entity and a numeric
                    // character reference resolve the same way, and an undefined entity
                    // stays an error rather than silently measuring as nothing.
                    let reconstructed = format!("&{name};");
                    let resolved = quick_xml::escape::unescape(&reconstructed)
                        .map_err(|e| PlsRejection::NotALexicon(e.to_string()))?;
                    *open = open.saturating_add(resolved.chars().count());
                }
            }
```
The comment under the guard claims "an undefined entity stays an error rather than silently measuring as nothing" — which holds only when `replacements.last_mut()` is `Some`. `replacements` is pushed only for `"alias" | "phoneme"` inside a `<lexeme>` (put_lexicon.rs:437-443), so in a `<grapheme>` the guard is `None` and the reference is dropped.

The same reference IS refused by the neighbouring arm when it sits outside the document element — put_lexicon.rs:491-496:
```
            // A CDATA section and a reference are character data too, so outside
            // every element they are the same refusal as text there — whatever they
            // hold, since neither has a whitespace-only form XML admits there.
            Event::CData(_) | Event::GeneralRef(_) if open_elements == 0 => {
                return Err(character_data_outside_the_document());
            }
```

The sibling XML reader in THIS crate runs the identical decode/unescape pair with NO guard — services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:740-753:
```
            // The reader BREAKS text at a reference and emits the reference on its
            // own, so a loop reading only text events would bill none of the
            // characters the document spells with `&amp;` or `&#NN;` — and those are
            // text content, which is billed, not markup, which is not.
            Event::GeneralRef(ref reference) => {
                let name = reference.decode().map_err(|e| e.to_string())?;
                // Reconstructed and unescaped so a predefined entity and a numeric
                // character reference resolve the same way, and a reference that
                // stands for nothing stays an error rather than billing as nothing.
                let reconstructed = format!("&{name};");
                let resolved =
                    quick_xml::escape::unescape(&reconstructed).map_err(|e| e.to_string())?;
                billed = billed.saturating_add(resolved.chars().count());
            }
```
and that `Err` becomes `InvalidSsmlException` at synthesize_speech.rs:1212-1216.

The crate states the rule as covering BOTH documents — services/service-com-amazonaws-polly-parrot-v1/tests/all/xml_well_formedness.rs:7-13:
```
//! RULE: a request carrying an XML document is accepted only when the whole content is
//! ONE well-formed XML document — exactly one document element, every element closed,
//! and nothing but whitespace, comments and processing instructions outside that
//! document element — so character data or a second root element before or after it is
//! refused with the document error of the operation that read it (`PutLexicon` ->
//! `InvalidLexiconException`, `SynthesizeSpeech` -> `InvalidSsmlException`) and nothing
//! of it is stored, billed or synthesized.
```

Test coverage: the SSML side is PINNED — tests/all/ssml_billed_characters.rs:301-316, `a_reference_standing_for_nothing_is_invalid_ssml`:
```
/// RULE (the same rule, where a reference stands for nothing): a reference the document
/// defines no replacement for is not well-formed text content, so the document is
/// `InvalidSsmlException` rather than a document whose unresolvable characters bill as
/// zero.
...
        ("an entity nothing declares", "a&nosuchentity;b", false),
        ("an entity nothing declares, alone", "&undefined;", false),
        ("a character reference with no digits", "a&#;b", false),
        ("a character reference past the code-point range", "a&#xFFFFFF;b", false),
```
The PLS side has NO equivalent row: `tests/all/xml_well_formedness.rs:47-58` only places `"&amp;"` and `"&#120;"` OUTSIDE the document element, and `tests/all/pls_replacement_measurement.rs:154-185` only uses RESOLVABLE references (`&amp;`, `&#120;`) and only INSIDE a replacement — so the unresolvable-reference-inside-the-document class is unpinned and nothing justifies the divergence.

## Steps to Reproduce

1. Boot the emulator and sign a Polly request for any Region.
2. PUT /v1/lexicons/Entity with body {"Content": "<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" version=\"1.0\" xml:lang=\"en-US\"><lexeme><grapheme>caf&nosuchentity;</grapheme><alias>cafe</alias></lexeme></lexicon>"} — observe HTTP 200: the `Event::GeneralRef` arm at put_lexicon.rs:508 finds `replacements.last_mut()` == None inside `<grapheme>`, so the unresolvable reference is dropped instead of refused.
3. GET /v1/lexicons/Entity — observe 200 with Lexicon.Content echoing the non-well-formed document verbatim and LexiconAttributes populated (LanguageCode en-US, LexemesCount 1, Size counting the reference's characters).
4. POST /v1/speech with {"OutputFormat":"mp3","VoiceId":"Joanna","TextType":"ssml","Text":"<speak>caf&nosuchentity;</speak>"} — observe 400 InvalidSsmlException for the SAME unresolvable reference, pinned by tests/all/ssml_billed_characters.rs:312.
5. Repeat step 2 with `&#;` and `&#xFFFFFF;` in the `<grapheme>` — both are accepted, while tests/all/ssml_billed_characters.rs:314-315 pins both as refusals on the SSML door.
6. Move the same reference inside the `<alias>` instead (`<alias>caf&nosuchentity;</alias>`) — now PutLexicon answers 400 InvalidLexiconException, so one document is refused and the other accepted purely by where the reference sits.
