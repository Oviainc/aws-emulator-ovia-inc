Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: d375777eceb36b3e90cf1faf12e0c90765e23519

# An unsupported phonetic alphabet declared on a <phoneme> element is stored: the alphabet check is applied only to the document element

_Filed: 2026-10-29T14:29:48Z_
_Operation: PutLexicon | Class: validation_

> _Note: this issue was identified against the code base at commit `d375777eceb36b3e90cf1faf12e0c90765e23519`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`parse_pls` checks a PLS document's `alphabet` against `SUPPORTED_PLS_ALPHABETS` in exactly one place — the document element (`<lexicon alphabet=...>`). PLS also lets a `<phoneme>` name its own alphabet, which the crate states twice in its own prose and relies on to justify treating a document element WITHOUT an `alphabet` as valid. But no code path ever looks at a `<phoneme>`'s `alphabet` attribute: in the per-element branch the only thing done with a `<phoneme>` is to resolve its attribute values as character data and push a replacement counter, so `<phoneme alphabet="klingon" ph="x">` is stored with HTTP 200 and `GetLexicon`/`ListLexicons` then report the lexicon as a valid one that declares no alphabet at all.

The same crate applies this very check on the sibling path: `SynthesizeSpeech`'s SSML reader declares `("alphabet", Domain::Keywords(PHONEME_ALPHABET))` for the `phoneme` element and refuses `<phoneme alphabet="klingon">` (pinned by `tests/all/ssml_vocabulary.rs`). So one reader in this crate validates a `<phoneme>`'s alphabet attribute and the other — the one whose operation models the alphabet error — skips it, letting a document carrying an alphabet outside the documented set through the check the crate already owns.

## Evidence

The model states the rule about the LEXICON's alphabet, not about one element of it — `smithy_query(query="doc", shape="com.amazonaws.polly#UnsupportedPlsAlphabetException")`:
"The alphabet specified by the lexicon is not a supported alphabet. Valid values are `ipa`, `x-sampa`, `x-amazon-pinyin`, `x-amazon-jyutping`, `x-amazon-pron-kana`, and `x-amazon-yomigana`. Replace the unsupported alphabet with one of the valid values and resubmit your request."

The crate says a `<phoneme>` may carry that attribute — services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs:202-203:
```
    /// The `<lexicon alphabet="...">` attribute, absent when the document omits it
    /// (PLS allows a per-phoneme alphabet instead).
```
and put_lexicon.rs:279-280:
```
/// `alphabet` is the one attribute PLS leaves optional here, because a `<phoneme>`
/// may name its own; it is read when present and checked by the caller.
```

The check is applied to the document element ONLY — put_lexicon.rs:602-606:
```
    if let Some(alphabet) = &document_element.alphabet {
        if !SUPPORTED_PLS_ALPHABETS.contains(&alphabet.as_str()) {
            return Err(PlsRejection::UnsupportedAlphabet(alphabet.clone()));
        }
    }
```

The per-element branch never consults `SUPPORTED_PLS_ALPHABETS` — put_lexicon.rs:486-509:
```
                check_attribute_values(element)?;
                match local_name.as_str() {
                    "lexeme" if in_pls_namespace => { ... }
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

The sibling reader in the same crate DOES check it — services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:448-449 and 538-544:
```
    /// `<phoneme alphabet>`.
    const PHONEME_ALPHABET: &[&str] = &["ipa", "x-sampa"];
...
        Element {
            name: "phoneme",
            attributes: &[
                ("alphabet", Domain::Keywords(PHONEME_ALPHABET)),
                ("ph", Domain::Text),
            ],
        },
```
pinned by services/service-com-amazonaws-polly-parrot-v1/tests/all/ssml_vocabulary.rs:282-285:
```
        (
            "phoneme alphabet",
            "<speak><phoneme alphabet=\"klingon\" ph=\"x\">hello</phoneme></speak>",
        ),
```

Test coverage stops exactly short of the hole: services/service-com-amazonaws-polly-parrot-v1/tests/all/pls_alphabet_support.rs:152-156 acknowledges the per-phoneme alphabet while asserting only the document-element case, and no row of `unsupported_alphabets()` (pls_alphabet_support.rs:53-65) puts the value on a `<phoneme>`:
```
    // PLS leaves `alphabet` off the document element when a `<phoneme>` names its own,
    // so a document without one is not a document with an unsupported one.
    let without = "<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" \
                   version=\"1.0\" xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme>\
                   <alias>World Wide Web Consortium</alias></lexeme></lexicon>";
```

## Steps to Reproduce

1. Boot the emulator and call `PUT /v1/lexicons/PhonemeAlpha` with body `{"Content": "<lexicon xmlns=\"http://www.w3.org/2005/01/pronunciation-lexicon\" version=\"1.0\" xml:lang=\"en-US\"><lexeme><grapheme>W3C</grapheme><phoneme alphabet=\"klingon\" ph=\"x\">w3c</phoneme></lexeme></lexicon>"}`.
2. Observe HTTP 200: the document element declares no `alphabet`, so put_lexicon.rs:602-606 checks nothing, and the `<phoneme>`'s `alphabet="klingon"` is never compared with `SUPPORTED_PLS_ALPHABETS`.
3. Call `GET /v1/lexicons/PhonemeAlpha`: the lexicon is stored and reported, with no `Alphabet` in `LexiconAttributes`.
4. Contrast 1 (same crate, same attribute, other reader): `POST /v1/speech` with `{"Text": "<speak><phoneme alphabet=\"klingon\" ph=\"x\">hello</phoneme></speak>", "TextType": "ssml", "OutputFormat": "mp3", "VoiceId": "Joanna"}` -> HTTP 400 `InvalidSsmlException` (pinned by tests/all/ssml_vocabulary.rs:282-285).
5. Contrast 2 (same operation, same value, document element): put the same alphabet on `<lexicon alphabet="klingon" ...>` -> HTTP 400 `InvalidLexiconException` and nothing stored (pinned by tests/all/pls_alphabet_support.rs:80-126).
