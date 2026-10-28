Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: 41cd94e3f39433730586ca0c36d19074975940e2

# PutLexicon measures a lexeme replacement with trim_text(true), so whitespace at a text-fragment edge is never counted

_Filed: 2026-10-28T14:55:16Z_
_Operation: PutLexicon | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `41cd94e3f39433730586ca0c36d19074975940e2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`parse_pls` configures its XML reader with `trim_text(true)` and then measures each `<alias>`/`<phoneme>` replacement by counting the characters of the text events it receives. quick-xml's trimming is applied PER TEXT EVENT, and the reader breaks text at every entity/character reference, so (a) whitespace at the start or end of a replacement is discarded before it is measured, and (b) whitespace adjacent to a reference inside the replacement is discarded too (a whitespace-only fragment counts as zero).

That contradicts the rule the function's own doc comment states — that the measurement is the WHOLE character content of the element and that no form of escaping can leave part of a replacement unmeasured — and it diverges from the sibling character counter in this crate, `SynthesizeSpeech`'s `billed_characters`, which reads the same kind of document with an untrimmed reader and counts every space (its test table bills "hello world" as 11).

The behavioural consequence is that a document whose replacement content is longer than the quota the handler enforces is accepted and stored with HTTP 200 instead of being refused with `MaxLexemeLengthExceededException`: the measurement under-counts, so the check it feeds is applied to a number that is not the replacement's character content. This is not a request to change the quota value — only to measure the content the handler says it measures.

## Evidence

The rule the code under it breaks — services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs:345-351:
```
/// Each `<lexeme>`'s replacement elements are measured as they
/// close, so an oversized one is refused without the document being stored; what is
/// measured is the WHOLE character content of the element — every text node under it,
/// whatever elements that text is nested in and whatever follows them, with a
/// character written as an entity or character reference counting as the one character
/// it stands for — so no nesting and no form of escaping can leave part of a
/// replacement unmeasured.
```

The reader it actually measures through — put_lexicon.rs:356-357:
```
    let mut reader = quick_xml::NsReader::from_str(content);
    reader.config_mut().trim_text(true);
```
and the per-fragment counting it feeds, put_lexicon.rs:439-449:
```
            Event::Text(ref text) => {
                if let Some(open) = replacements.last_mut() {
                    let raw = String::from_utf8_lossy(text.as_ref()).to_string();
                    let unescaped = quick_xml::escape::unescape(&raw)...;
                    *open = open.saturating_add(unescaped.chars().count());
```

The sibling counter in the SAME crate, with no trimming — services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs:694-695:
```
    use quick_xml::events::Event;
    let mut reader = quick_xml::Reader::from_str(text);
```
and its test pinning that spaces ARE counted there — tests/all/ssml_billed_characters.rs:46-51:
```
            case: "flat literal text",
            inner: "hello world".to_string(),
            plain: "hello world".to_string(),
            billed: 11,
```

That per-fragment trimming eats a value's own spaces is documented in the framework this crate runs on — vendor/smithy-schema-server/src/xml.rs:1113-1116:
```
        // NOT `trim_text(true)`. The reader breaks text at every entity reference, so a value
        // arrives as several fragments and per-fragment trimming would eat the value's own
        // interior spaces: `a &amp; b` would deserialize as `a&b`.
```

Test coverage: tests/all/pls_replacement_measurement.rs:200 `a_replacement_is_measured_over_its_whole_content` spans nesting, CDATA and entity references but carries NO row whose replacement holds whitespace, so neither case above is pinned by any test.

## Steps to Reproduce

1. PUT /v1/lexicons/Pad with a PLS 1.0 document whose single lexeme is `<lexeme><grapheme>G</grapheme><alias>` + "x"*100 + "          " + `</alias></lexeme>` — 110 characters of replacement content.
2. Observe HTTP 200 and GET /v1/lexicons/Pad returning the document: the trailing whitespace was trimmed away before measuring, so the replacement measured 100 and passed MAX_LEXEME_CHARACTERS.
3. PUT /v1/lexicons/Ref with `<alias>` + "x"*98 + " &amp; " + `</alias>`: the content is 101 characters (98 x, space, &, space) but the fragments measure 98 + 1 + 0 = 99, so it is stored instead of refused with MaxLexemeLengthExceededException.
4. Compare with SynthesizeSpeech, which counts the same characters untrimmed: POST /v1/speech with TextType=ssml and Text=`<speak>hello world</speak>` reports RequestCharacters 11, the space included (tests/all/ssml_billed_characters.rs:46-51).
