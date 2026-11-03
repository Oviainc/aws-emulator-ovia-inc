Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: translate.translate-document-plain-text-round-trip
Issue-Shape-Id: com.amazonaws.translate#AWSShineFrontendService_20170701
Issue-Key: translate.translate-document-plain-text-round-trip|run#13|AssertionError|assert not untranslated, (
Issue-Commit: d4f8ce799a785ffcb655def193d42db12684ea89

# TranslateDocument does not translate: the response document is the source bytes with a synthetic [mt:...] marker prepended

> _Note: this issue was identified against the code base at commit `d4f8ce799a785ffcb655def193d42db12684ea89`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TranslateDocument

## Description
One net-new root cause in Amazon Translate's synchronous document data plane: TranslateDocument performs no translation. The bytes it returns in TranslatedDocument.Content are the request's own document content (after optional custom-terminology term substitution) with an internal marker string prepended, so the returned document is still in the SOURCE language.

Cause 1 of 1 — TranslateDocument returns the untranslated source document, marked instead of translated.

AWS: Smithy: TranslateDocument input Document is @required (Document.Content, documented "The content and content type for the document to be translated. The document size must not exceed 100 KB."), SourceLanguageCode and TargetLanguageCode are @required (LanguageCodeString, @length 2..5); output TranslatedDocument is @required and its Content member is documented "The document containing the translated content." The operation is documented "Translates the input document from the source language to the target language. This synchronous operation supports text, HTML, or Word documents as the input document." and "The document format matches the source document format." The contract for a supported pair (TranslateDocument supports en -> any supported language and any supported language -> en) is therefore that the returned document's text is in the TARGET language: the source-language content words of the request do not survive verbatim in the output, while the document's format/markup is preserved. Status 200 on both sides; only the returned document body diverges.

Emulator: for a text/plain document whose content is an English sentence, translated en -> es, the response document decodes to the English source verbatim with a bracketed marker in front — e.g. '[mt:en->es] Good morning, colleague. The quarterly report is ready.' for the source 'Good morning, colleague. The quarterly report is ready.'. Every English content word ('colleague', 'quarterly', 'report', 'ready') is still present. In the checkout the handler builds the answer from `translation_marker(&source_language_code, &target_language_code, applied_settings)` and `Translation::segment()`, which returns `format!("{marker}{translated}")` where `translated` comes from `substitute(text, self.pairs, self.budget)` — i.e. the input text unchanged unless a named custom terminology happens to match a term. With no TerminologyNames the document content is copied through byte for byte, for all three content types (the plain-text, HTML and Word paths all route their segments through the same `segment()`).

Why: a client that uploads a document for synchronous translation gets its own untranslated document back, with content that is not valid output for the declared TargetLanguageCode, plus a marker string injected into the document's first text segment that corrupts the document body itself (in an HTML or Word document the marker is inserted into the first text node/run). Because the response still reports SourceLanguageCode/TargetLanguageCode as requested and returns 200, the caller cannot distinguish "translated" from "not translated": any pipeline that stores, publishes, indexes, diffs or language-checks the translated document silently produces wrong data instead of failing. It also makes TranslateDocument inconsistent with any real translation expectation shared with TranslateText.

Fix: the TranslateDocument handler's text-production path in the translate service crate — the marker construction and the per-segment translation step that currently answers with the substituted source text must emit target-language text for the resolved language pair, and must not prepend a marker into the document's content. The surrounding behavior the handler already gets right (content-type dispatch, markup preservation, terminology resolution/AppliedTerminologies, size limits) should be kept.

Related open issues that do NOT cover this: `20261029T192448Z-translate-translatetext-returns-the-source-text-with-a-synthetic.md` is scoped to the TranslateText operation and its Settings; `20261029T184304Z-translate-success-payloads-carry-wrong-terminology-arn-and.md` covers only the cosmetic `[mt:...]` prefix as a wire-field comparison on TranslateText's `$.TranslatedText`; `20261103T092830Z-translate-translatedocument-not-found-error-answers-http-404-where.md` covers TranslateDocument's not-found ERROR status only. Removing the marker alone would still leave the untranslated source document in TranslatedDocument.Content.

Traces: translate.translate-document-plain-text-round-trip.

## Evidence
Replay of behavioral suite simparity-behavioral-aws-v1 against the emulator (translate 12/20 passed; bootstrap_signal=false, so the emulator was serving and answered this call with HTTP 200).

Cause 1 — [mismatch] translate.translate-document-plain-text-round-trip:
  AssertionError: English content words survived untranslated: ['colleague', 'quarterly', 'report', 'ready'] in '[mt:en->es] Good morning, colleague. The quarterly report is ready.'

  Request: TranslateDocument(Document={Content=<utf-8 bytes of an English sentence>, ContentType='text/plain'}, SourceLanguageCode='en', TargetLanguageCode='es').
  Expected (AWS): HTTP 200 with SourceLanguageCode='en', TargetLanguageCode='es', and TranslatedDocument.Content decoding to Spanish text — non-empty, not byte-identical to the source, and containing none of the source's English content words.
  Got (emulator): HTTP 200 with the correct SourceLanguageCode/TargetLanguageCode, but TranslatedDocument.Content decodes to the English source verbatim behind a '[mt:en->es] ' marker, so all four English content words are still present.

Source of the behavior in the checkout: services/service-com-amazonaws-translate-awsshinefrontendservice-20170701/src/ops/translate_document.rs — `translation_marker(...)` call (lines 706-710), `Translation::new(pairs, marker)` (line 720), and `Translation::segment()` returning `format!("{marker}{translated}")` (lines 246-267) over `substitute(text, self.pairs, self.budget)` (line 247); the plain-text path `Ok(translate(text).into_bytes())` (line 476) and the markup/Word paths all feed the same `segment()`.

(The specific sentence above is the check's input; the requirement is the general behavior — the returned document's text is in the requested target language, in the source document's format, with no marker injected into its content.)

## Steps to Reproduce
1. git clone the repo and boot the emulator built from it.
2. Replay behavioral suite simparity-behavioral-aws-v1 (run_behavioral_suite) and re-run the check in trace_ids: translate.translate-document-plain-text-round-trip.
3. Directly: call TranslateDocument with Document={'Content': b'Good morning, colleague. The quarterly report is ready.', 'ContentType': 'text/plain'}, SourceLanguageCode='en', TargetLanguageCode='es'.
4. Assert the response is 200 with SourceLanguageCode='en' and TargetLanguageCode='es'.
5. Decode TranslatedDocument.Content as UTF-8 and assert it is non-empty, differs from the source bytes, carries no '[mt:' marker, and contains none of the source's English content words ('colleague', 'quarterly', 'report', 'ready'). Today the decoded content is '[mt:en->es] ' + the unchanged English source, so step 5 fails.
