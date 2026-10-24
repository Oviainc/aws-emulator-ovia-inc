Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: cdf8bb57609ff190f9228b1bff313b67e030c44d

# PutLexicon accepts a PLS document that omits the attributes PLS 1.0 requires (version, xml:lang, the PLS namespace)

_Filed: 2026-10-24T08:35:23Z_
_Operation: PutLexicon | Class: validation_

> _Note: this issue was identified against the code base at commit `cdf8bb57609ff190f9228b1bff313b67e030c44d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The PLS reader only checks that the document element is named `lexicon`; everything else about the document element is optional. A document such as `<lexicon/>` or `<lexicon><lexeme><grapheme>a</grapheme><alias>b</alias></lexeme></lexicon>` is therefore stored with HTTP 200.

Expected: a document whose `<lexicon>` element omits the PLS-required `version="1.0"` / `xml:lang` attributes (and the `xmlns="http://www.w3.org/2005/01/pronunciation-lexicon"` namespace Polly's PLS reader expects) is not a valid PLS 1.0 lexicon, and Amazon Polly refuses it with HTTP 400 `InvalidLexiconException` (fixed message "Invalid PLS Lexicon") without storing anything. The language is not optional in practice: Polly applies a lexicon "only if the language of the lexicon is the same as the language of the voice", so a lexicon with no language could never be applied, and every real GetLexicon response carries `LexiconAttributes.LanguageCode`.

Actual: src/ops/put_lexicon.rs `parse_pls` leaves `alphabet` and `language_code` as `None` when the attributes are absent (lines 258-274) and never checks `version` or the namespace; the handler then stores the document and GetLexicon/ListLexicons report a lexicon whose `LexiconAttributes` has no `LanguageCode` member at all — a state real Polly cannot produce.

## Evidence

Spec grounding: PutLexicon's own documentation requires the content to be a valid PLS document and the model documents `UnsupportedPlsLanguageException` plus `LexiconAttributes$LanguageCode` ("A lexicon with a language code such as \"en\" would be applied to all English languages"), which presupposes the document declares one; PLS 1.0 declares `version` and `xml:lang` required on the `lexicon` element, and every example in the Polly lexicon documentation carries version, the PLS xmlns and xml:lang. Handler: src/ops/put_lexicon.rs lines 245-275 (only the local name of the root is validated; attributes other than `alphabet` and `xml:lang` are ignored) and lines 326-341 (the alphabet/language checks are skipped entirely when the attribute is absent). Test coverage: every PLS document in src/ops/put_lexicon.rs tests and in tests/all/*.rs carries both version and xml:lang (e.g. tests/all/lexicons_and_speech.rs `pls()` at line 131, tests/all/error_message_wording.rs line 83), so no test asserts acceptance OR rejection of a document missing them — the gap is uncovered.

## Steps to Reproduce

1. Boot the emulator and call PUT /v1/lexicons/NoLang with body {"Content": "<lexicon><lexeme><grapheme>W3C</grapheme><alias>World Wide Web Consortium</alias></lexeme></lexicon>"}
2. Observe HTTP 200, where real Polly answers HTTP 400 InvalidLexiconException with message "Invalid PLS Lexicon"
3. Call GET /v1/lexicons/NoLang and observe 200 with LexiconAttributes that carry no LanguageCode member
4. Repeat with `<lexicon version="1.0"/>` (no xml:lang) and with a document whose root carries no PLS namespace — both are stored instead of refused
