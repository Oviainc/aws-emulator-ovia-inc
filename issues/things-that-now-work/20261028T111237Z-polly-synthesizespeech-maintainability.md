Issue-Severity: LOW
Issue-Gap-Class: maintainability
Issue-Commit: fa255a5b72d32034e1d300bb9323da879d85edac

# The XML character-counting rule is implemented twice (SSML billing and PLS replacement measurement)

_Filed: 2026-10-28T11:12:37Z_
_Operation: SynthesizeSpeech | Class: maintainability_

> _Note: this issue was identified against the code base at commit `fa255a5b72d32034e1d300bb9323da879d85edac`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The rule "how many characters does this XML event contribute" — unescape a Text event before counting, count CDATA verbatim, and reconstruct a GeneralRef as `&name;` and unescape it so an entity or numeric character reference counts as the one character it stands for — is written out twice in this crate, once in `SynthesizeSpeech`'s `billed_characters` and once in `PutLexicon`'s `parse_pls`. The two copies are character-for-character the same decision (including the same explanatory comment about the reader breaking text at a reference), differing only in which accumulator they add to: `billed` in one and the innermost open replacement counter in the other.

Because the rule lives in two places, the two can drift: a future change to how one of them treats a reference, a CDATA section or a malformed entity silently applies to only one of `SynthesizeSpeech`'s billed-character count and `PutLexicon`'s lexeme-length measurement, even though both are the same "count the resolved characters of XML text content" rule. The fix is to count a single `quick_xml` event through one shared helper in the crate and have both loops add its result to their own accumulator; every response and every existing test stays byte-identical, because the arithmetic is unchanged.

## Evidence

Copy 1 — services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:708-733 (inside `billed_characters`):
```
            Event::Text(ref t) => {
                // Entities count as the character they stand for, so the raw bytes
                // are unescaped before counting.
                let raw = String::from_utf8_lossy(t.as_ref()).to_string();
                let decoded =
                    quick_xml::escape::unescape(&raw).map_err(|e| e.to_string())?;
                billed = billed.saturating_add(decoded.chars().count());
            }
            Event::CData(ref c) => {
                billed = billed
                    .saturating_add(String::from_utf8_lossy(c.as_ref()).chars().count());
            }
            // The reader BREAKS text at a reference and emits the reference on its
            // own, so a loop reading only text events would bill none of the
            // characters the document spells with `&amp;` or `&#NN;` ...
            Event::GeneralRef(ref reference) => {
                let name = reference.decode().map_err(|e| e.to_string())?;
                let reconstructed = format!("&{name};");
                let resolved =
                    quick_xml::escape::unescape(&reconstructed).map_err(|e| e.to_string())?;
                billed = billed.saturating_add(resolved.chars().count());
            }
```

Copy 2 — services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs:439-475 (inside `parse_pls`):
```
            Event::Text(ref text) => {
                // The innermost open replacement counts the text; ...
                if let Some(open) = replacements.last_mut() {
                    // Entities stand for the character they name, so the raw bytes
                    // are unescaped before the replacement is measured.
                    let raw = String::from_utf8_lossy(text.as_ref()).to_string();
                    let unescaped = quick_xml::escape::unescape(&raw)
                        .map_err(|e| PlsRejection::NotALexicon(e.to_string()))?;
                    *open = open.saturating_add(unescaped.chars().count());
                }
            }
            Event::CData(ref text) => {
                if let Some(open) = replacements.last_mut() {
                    let characters = String::from_utf8_lossy(text.as_ref()).chars().count();
                    *open = open.saturating_add(characters);
                }
            }
            // The reader BREAKS text at a reference and emits the reference on its own,
            // so a loop reading only text events would measure none of the characters a
            // replacement spells with `&amp;` or `&#NN;`.
            Event::GeneralRef(ref reference) => {
                if let Some(open) = replacements.last_mut() {
                    let name = reference
                        .decode()
                        .map_err(|e| PlsRejection::NotALexicon(e.to_string()))?;
                    let reconstructed = format!("&{name};");
                    let resolved = quick_xml::escape::unescape(&reconstructed)
                        .map_err(|e| PlsRejection::NotALexicon(e.to_string()))?;
                    *open = open.saturating_add(resolved.chars().count());
                }
            }
```

Both copies are covered by existing end-to-end suites, which must keep passing unchanged:
- services/service-com-amazonaws-polly-parrot-v1/tests/all/ssml_billed_characters.rs:152 `references_bill_as_the_characters_they_stand_for`, :219 `the_billed_limit_is_measured_on_the_resolved_characters`, :306 `a_reference_standing_for_nothing_is_invalid_ssml`
- services/service-com-amazonaws-polly-parrot-v1/tests/all/pls_replacement_measurement.rs:200 `a_replacement_is_measured_over_its_whole_content`, :266 `one_oversized_replacement_among_many_refuses_the_document`
- the in-module unit test services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:1465 `ssml_tags_are_not_billed`

## Steps to Reproduce

1. Open services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs and read the `Event::Text` / `Event::CData` / `Event::GeneralRef` arms of `billed_characters` (lines 708-733).
2. Open services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs and read the `Event::Text` / `Event::CData` / `Event::GeneralRef` arms of `parse_pls` (lines 439-475).
3. Observe the two arms implement the same unescape/CDATA/reconstruct-reference counting rule, with the same comment, differing only in the accumulator they add to — so a change to the rule has to be made in two places or the two measurements drift.
4. Confirm the behaviour is pinned by tests/all/ssml_billed_characters.rs and tests/all/pls_replacement_measurement.rs, which must stay unchanged after the duplication is collapsed into one helper.
