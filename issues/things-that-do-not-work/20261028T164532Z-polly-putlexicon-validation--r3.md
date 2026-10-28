Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 0b60124a277040298c4e92ee09b21d27ce53d94f

# parse_pls judges only the FIRST element, so content outside the document element and a second root element are stored as a valid PLS lexicon

_Filed: 2026-10-28T16:45:32Z_
_Operation: PutLexicon | Class: validation_

> _Note: this issue was identified against the code base at commit `0b60124a277040298c4e92ee09b21d27ce53d94f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutLexicon`'s PLS reader (`parse_pls`) performs NO structural XML well-formedness check. It calls `read_document_element` on the first `Start`/`Empty` event it sees and then `continue`s; every later event that is not a PLS-namespaced `lexeme`/`alias`/`phoneme` falls into a `_ => {}` arm, and every `Event::Text` outside an open replacement is silently discarded. The only whole-document check at the end is "the content carries no XML element".

Three classes of document that are not well-formed XML (XML 1.0 admits exactly one root element and no character data outside it) are therefore accepted, stored, and reported by `GetLexicon`/`ListLexicons` as a valid lexicon with derived attributes:

1. Character data BEFORE the document element: `garbage<lexicon xmlns="…pronunciation-lexicon" version="1.0" xml:lang="en-US"/>` — the `Event::Text("garbage")` hits the `replacements.last_mut()` guard, which is `None`, so it is dropped; the following `<lexicon>` becomes the document element and the put succeeds.
2. Character data AFTER the document element: `<lexicon …/>garbage` — same path, dropped.
3. A SECOND sibling root element after the document element: `<lexicon …/><speak>hello</speak>` — `document_element.is_none()` is false by then, so the element goes to the `match local_name` whose `_ => {}` arm ignores it.

The crate's own rule is the opposite on both counts. `parse_pls`'s doc comment states that "any malformed one is refused", and the crate already refuses the standalone equivalents: `<speak>hello</speak>` alone is `InvalidLexiconException` (because it IS the first element) and `<lexicon><lexicon xmlns=… version=… xml:lang=…>…</lexicon></lexicon>` is refused because only the document element counts. So `<speak>hello</speak>` is refused when it comes first and accepted when it comes second, for the same byte string in `Content`.

The asymmetry is visible inside the crate as well: the sibling XML reader `billed_characters` in `synthesize_speech.rs` tracks an element `depth` and refuses `depth != 0` with "an SSML element is left unclosed". `parse_pls` tracks no element depth at all (only `lexeme_depth` and the `replacements` stack), so it has no equivalent structural guard.

Consequence: the store accepts a `Content` that is not a PLS document. `StoredLexicon.size` counts the stray characters, and `GetLexicon` hands the whole non-well-formed blob back verbatim as `Lexicon$Content`.

## Evidence

The doc comment the code under it breaks — services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs:341-343:
```
/// The reader is a real, NAMESPACE-AWARE XML parser, so any well-formed PLS document
/// is accepted whatever prefix it spells its elements with, and any malformed one is
/// refused — the shape of the examples this emulator has seen does not enter into it.
```

Only the FIRST element is judged — put_lexicon.rs:384-391:
```
                if document_element.is_none() {
                    document_element = Some(read_document_element(
                        element,
                        in_pls_namespace,
                        &local_name,
                    )?);
                    continue;
                }
```

Every later element that is not a PLS lexeme/replacement is ignored — put_lexicon.rs:395-414 (the fall-through arm at :413):
```
                match local_name.as_str() {
                    "lexeme" if in_pls_namespace => { … }
                    "alias" | "phoneme"
                        if in_pls_namespace
                            && lexeme_depth > 0
                            && matches!(event, Event::Start(_)) =>
                    {
                        replacements.push(0usize);
                    }
                    _ => {}
                }
```

Text outside an open replacement is discarded — put_lexicon.rs:439-449:
```
            Event::Text(ref text) => {
                // The innermost open replacement counts the text; the elements around
                // it count it when that one closes.
                if let Some(open) = replacements.last_mut() {
```

The only whole-document check — put_lexicon.rs:480-482:
```
    let document_element = document_element.ok_or_else(|| {
        PlsRejection::NotALexicon("the content carries no XML element".to_string())
    })?;
```

The sibling XML reader in this crate that DOES carry a structural check — services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:740-742:
```
    if depth != 0 {
        return Err("an SSML element is left unclosed".to_string());
    }
```

The crate refuses the same byte strings when they come FIRST — services/service-com-amazonaws-polly-parrot-v1/tests/all/pls_alphabet_support.rs:188-191:
```
            "another vocabulary's document".to_string(),
            "<speak>hello</speak>".to_string(),
            "InvalidLexiconException",
```
and services/service-com-amazonaws-polly-parrot-v1/tests/all/pls_document_validity.rs:100-106:
```
            "every declaration on a child instead of the document element",
            format!(
                "<lexicon><lexicon xmlns=\"{PLS_NAMESPACE}\" version=\"{PLS_VERSION}\" \
                 xml:lang=\"en-US\">{LEXEME}</lexicon></lexicon>"
            ),
```

Test coverage: `pls_document_validity.rs` spans documents whose DOCUMENT ELEMENT is wrong (absent namespace/version/xml:lang, foreign namespace, declarations on a child, the XML declaration alone) and `pls_alphabet_support.rs::the_unusable_document_codes_are_the_ones_amazon_polly_answers` spans "not XML at all" (`"nonsense <"`). No row in either table puts character data or a second root element OUTSIDE the document element, so this class is unpinned.

## Steps to Reproduce

1. Boot the emulator and sign a Polly request for any Region.
2. PUT /v1/lexicons/Trailing with body {"Content": "<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" version=\"1.0\" xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme><alias>World Wide Web Consortium</alias></lexeme></lexicon><speak>hello</speak>"} — observe HTTP 200 instead of 400 InvalidLexiconException, although the same `<speak>hello</speak>` alone is refused (pls_alphabet_support.rs:188-191).
3. PUT /v1/lexicons/Leading with body {"Content": "garbage<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" version=\"1.0\" xml:lang=\"en-US\"/>"} — observe HTTP 200: the leading character data is dropped by the `Event::Text` guard at put_lexicon.rs:442.
4. GET /v1/lexicons/Trailing — observe 200 with Lexicon.Content echoing the non-well-formed document verbatim and LexiconAttributes populated (LanguageCode en-US, LexemesCount 1, Size counting the stray characters).
5. GET /v1/lexicons — observe both non-PLS documents listed as stored lexicons.
