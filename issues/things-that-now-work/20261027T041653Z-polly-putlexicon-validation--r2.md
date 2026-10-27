Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 11fd98189f90040dc28179001c4adbbf7cfc6f77

# PutLexicon's lexeme-length check is skipped when one replacement element contains another

_Filed: 2026-10-27T04:16:53Z_
_Operation: PutLexicon | Class: validation_

> _Note: this issue was identified against the code base at commit `11fd98189f90040dc28179001c4adbbf7cfc6f77`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`parse_pls` enforces the per-lexeme replacement quota by opening an accumulator on the START of an `<alias>`/`<phoneme>` inside a `<lexeme>` and measuring it on that element's END. Opening is unconditional: a START of `<alias>`/`<phoneme>` while a replacement accumulator is already open REPLACES it with a fresh empty String, throwing away everything accumulated so far without ever measuring it. The outer element's own END then finds `replacement == None` (the inner END already `take()`-d it) and measures nothing at all.

Consequence: a PLS document whose `<alias>` carries 200 characters of text and then a nested `<phoneme>` is stored with HTTP 200 instead of being refused with `MaxLexemeLengthExceededException`, even though the handler's own doc states that rule and the crate's `MAX_LEXEME_CHARACTERS` constant is 100. Text that follows the inner element inside the outer one is dropped from the measurement for the same reason (the accumulator is `None` again, so later `Event::Text` is not collected). `Content` carries no modelled constraint, so this document reaches the handler.

## Evidence

The rule the handler states for itself — services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs:25-27: "a `<lexeme>` whose replacement is longer than [`MAX_LEXEME_CHARACTERS`] is `MaxLexemeLengthExceededException`", and put_lexicon.rs:344-346: "Each `<lexeme>`'s replacement elements are measured as they close, so an oversized one is refused without the document being stored."

The code that breaks it — put_lexicon.rs:394-398 (START, unconditional reset):
```
                    "alias" | "phoneme"
                        if in_pls_namespace && in_lexeme && matches!(event, Event::Start(_)) =>
                    {
                        replacement = Some(String::new());
                    }
```
put_lexicon.rs:407-414 (END, measures only what is still open):
```
                    "alias" | "phoneme" if in_pls_namespace => {
                        if let Some(text) = replacement.take() {
                            let length = text.chars().count();
                            if length > MAX_LEXEME_CHARACTERS {
                                return Err(PlsRejection::LexemeTooLong(length));
```
and the quota it is supposed to apply, put_lexicon.rs:165: `pub const MAX_LEXEME_CHARACTERS: usize = 100;`

Test coverage is flat-only, so the path is unexercised: tests/all/error_semantics.rs:620 `put_lexicon_enforces_the_lexeme_length_quota` builds every row with tests/all/error_semantics.rs:72-80 `pls_with_replacement`, which emits exactly one replacement element per lexeme — "<lexeme><grapheme>G</grapheme><{element}>{}</{element}></lexeme>" — and tests/all/error_message_wording.rs:74-78 `pls_with` does the same ("<lexeme><grapheme>G{i}</grapheme><{element}>{value}</{element}></lexeme>"). No row nests one replacement element inside another.

## Steps to Reproduce

1. Boot the emulator with the Polly service mounted.
2. Call PutLexicon with Name="Nested" and Content = `<lexicon xmlns="http://www.w3.org/2005/01/pronunciation-lexicon" version="1.0" alphabet="ipa" xml:lang="en-US"><lexeme><grapheme>G</grapheme><alias>` + "x"*200 + `<phoneme>z</phoneme></alias></lexeme></lexicon>`.
3. Expected (the rule put_lexicon.rs:25-27 and :344-346 state, with MAX_LEXEME_CHARACTERS = 100): HTTP 400 `MaxLexemeLengthExceededException` and nothing stored.
4. Actual: HTTP 200, and GetLexicon(Name="Nested") returns the document — the 200-character alias replacement was discarded by the nested `<phoneme>` START at put_lexicon.rs:397 and never measured.
5. Compare the same document with the nested `<phoneme>` removed (`<alias>` + "x"*200 + `</alias>`): that one is correctly refused with 400 MaxLexemeLengthExceededException, which shows the only difference is the nesting.
