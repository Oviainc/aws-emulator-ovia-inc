Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# UpdateVocabulary refuses a VocabularyFileUri update with 400 instead of rebuilding the vocabulary

_Filed: 2026-10-24T01:59:20Z_
_Operation: UpdateVocabulary | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateVocabulary with `VocabularyFileUri` (and no `Phrases`) is refused with HTTP 400 BadRequestException and the stored vocabulary is left untouched. The real service accepts it, overwrites the vocabulary's content, answers HTTP 200 with `VocabularyState: PENDING`, and the vocabulary returns to READY once rebuilt.

Expected: 200 with `{VocabularyName, LanguageCode, LastModifiedTime, VocabularyState: "PENDING"}` and the record's LastModifiedTime/state advanced.
Actual: 400 BadRequestException ("The VocabularyFileUri <uri> can't be read by this emulator: it rebuilds a custom vocabulary only from the terms a request lists in Phrases"); no state change at all, so a caller that re-uploads a term file sees neither the new LastModifiedTime nor the PENDING->READY transition.

## Evidence

Spec: `UpdateVocabulary` input models `VocabularyFileUri` (Uri, len 1..2000, pattern ^(s3://|http(s*)://).+$) alongside `Phrases` (smithy_query operation=UpdateVocabulary), and the operation doc describes the same two alternatives as the create. Handler: src/ops/update_vocabulary.rs lines 215-243, `ContentSource::File(uri)` -> `bad_request(...)`, returning before the record mutation at lines 273-281. Since the service reports no terms on any output shape, the file branch can be recorded rather than refused. No test asserts the accepting behavior (tests/all/vocabularies.rs only covers the inline-Phrases update path).

## Steps to Reproduce

1. CreateVocabulary with inline Phrases and wait for READY
2. POST / with X-Amz-Target: Transcribe.UpdateVocabulary and body {"VocabularyName":"<name>","LanguageCode":"en-US","VocabularyFileUri":"s3://example-bucket/vocab.txt"}
3. Observe HTTP 400 BadRequestException instead of HTTP 200 with VocabularyState PENDING
4. Call GetVocabulary — LastModifiedTime and VocabularyState are unchanged, where AWS reports a new LastModifiedTime and PENDING
