Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: afab077872ee6c32a9626f154d3b33fea96b8a18

# A lexicon whose xml:lang differs only in case is stored and read back with a LanguageCode outside the model's enum, an enum widening the check's own doc does not state

_Filed: 2026-10-28T18:16:58Z_
_Operation: GetLexicon | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `afab077872ee6c32a9626f154d3b33fea96b8a18`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutLexicon`'s language gate, `language_is_supported`, compares the document's `xml:lang` against the model's `LanguageCode` enum with `eq_ignore_ascii_case` in BOTH of its branches. Its own doc comment states exactly ONE widening of the enum and gives the reason for it (a bare primary subtag such as `en`, because `LexiconAttributes$LanguageCode`'s documentation blesses it). Case-insensitivity is a SECOND, unstated widening: `xml:lang="EN-us"`, `xml:lang="Fr-FR"` or `xml:lang="EN"` all pass the gate instead of being refused with `UnsupportedPlsLanguageException`.

Nothing downstream normalises the value: `PutLexicon` stores the document's spelling verbatim (`language_code: parsed.language_code`) and the shared reporter `crate::state::lexicon_attributes` hands that same string back, so `GetLexicon` (and `ListLexicons`, which shares the helper) answers 200 with `LexiconAttributes.LanguageCode = "EN-us"` — a value the modelled `LanguageCode` enum this member targets does not carry, and not the documented primary-subtag form either. An SDK that deserialises the member as the enum gets an unknown variant for a lexicon the service itself accepted.

The crate enforces the opposite invariant on its other reported-language path: the voice catalog is asserted, with a stated reason, to carry only modelled `LanguageCode` values. The lexicon path applies no such normalisation or check.

## Evidence

The check and its own doc, `services/service-com-amazonaws-polly-parrot-v1/src/ops/put_lexicon.rs:500`:
```
/// Whether `language` is a language Amazon Polly supports.
///
/// The catalog is the model's own `LanguageCode` enum. A bare primary subtag counts:
/// `LexiconAttributes$LanguageCode`'s documentation says "A lexicon with a language
/// code such as \"en\" would be applied to all English languages (en-GB, en-US,
/// en-AUS, en-WLS, and so on", so `en` is a language a lexicon may declare even
/// though it is not itself an enum value.
fn language_is_supported(language: &str) -> bool {
    LANGUAGE_CODE_VALUES.iter().any(|code| {
        code.eq_ignore_ascii_case(language)
            || code
                .split('-')
                .next()
                .is_some_and(|primary| primary.eq_ignore_ascii_case(language))
    })
}
```
The doc states one widening (the bare primary subtag); the code's `eq_ignore_ascii_case` in both branches adds case-folding, which the doc does not state.

The value is stored verbatim — `put_lexicon.rs:580`: `language_code: parsed.language_code,` — and reported verbatim by the shared helper, `src/state.rs:408`:
```
pub fn lexicon_attributes(stored: &StoredLexicon) -> crate::ops::types::LexiconAttributes {
    crate::ops::types::LexiconAttributes {
        alphabet: stored.alphabet.clone(),
        language_code: Some(stored.language_code.clone()),
```
reached by `GetLexicon` at `src/ops/get_lexicon.rs:172`: `lexicon_attributes: Some(crate::state::lexicon_attributes(stored)),`.

The member is an ENUM target: `smithy_query(query="shape", shape="LexiconAttributes")` reports `"LanguageCode": {"target": "LanguageCode", "type": "enum"}`, and `LANGUAGE_CODE_VALUES` (src/ops/types/shared_types.rs:114) is that enum: `["ar-AE", "arb", ..., "en-US", ...]` — no `"EN-us"`.

The crate's own invariant for the other reported-language path, `src/state.rs:666`:
```
/// Every catalog row must name values the model declares, or `DescribeVoices`
/// would serve a voice no SDK can deserialize.
#[test]
fn every_seeded_voice_uses_modeled_enum_values() {
    ...
        assert!(
            LANGUAGE_CODE_VALUES.contains(&voice.language_code),
```

Test coverage: the existing unit test `the_declared_language_is_what_the_document_reports` (put_lexicon.rs:794) and `a_primary_language_subtag_is_a_supported_language` (put_lexicon.rs:894) only use canonically spelled values (`"en-US"`, `"en"`, `"en-GB"`, `"arb"`, `"cmn-CN"`, `"ja-JP"`, and rejections `"xx"`, `"klingon"`, `""`), and the end-to-end suite `tests/all/pls_document_validity.rs:263` only asserts that SOME `LanguageCode` is present — no test exercises a case-variant, so no test pins the current answer.

## Steps to Reproduce

1. PUT /v1/lexicons/Mixed with Content `<lexicon xmlns="http://www.w3.org/2005/01/pronunciation-lexicon" version="1.0" alphabet="ipa" xml:lang="EN-us"><lexeme><grapheme>W3C</grapheme><alias>World Wide Web Consortium</alias></lexeme></lexicon>`
2. Observe 200: `language_is_supported("EN-us")` matches `"en-US"` under `eq_ignore_ascii_case`, so the document is stored instead of being refused with UnsupportedPlsLanguageException
3. GET /v1/lexicons/Mixed
4. Observe `LexiconAttributes.LanguageCode == "EN-us"` — not a value of the modelled `LanguageCode` enum the member targets, and not the documented bare-primary-subtag form either
5. GET /v1/lexicons and observe the same out-of-enum value in the listed description (both readers share `crate::state::lexicon_attributes`)
