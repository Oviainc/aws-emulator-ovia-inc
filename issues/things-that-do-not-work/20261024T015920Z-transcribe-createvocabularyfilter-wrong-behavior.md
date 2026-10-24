Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# CreateVocabularyFilter refuses a VocabularyFilterFileUri create with 400 instead of recording the filter

_Filed: 2026-10-24T01:59:20Z_
_Operation: CreateVocabularyFilter | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

A CreateVocabularyFilter request that supplies its words through `VocabularyFilterFileUri` is refused with HTTP 400 BadRequestException and nothing is stored. The real service accepts it, answers HTTP 200 with `{VocabularyFilterName, LanguageCode, LastModifiedTime}`, and the filter is then reachable through GetVocabularyFilter / ListVocabularyFilters / UpdateVocabularyFilter / DeleteVocabularyFilter and usable from `Settings.VocabularyFilterName`.

Expected: 200 and a stored filter record.
Actual: 400 BadRequestException ("The VocabularyFilterFileUri <uri> can't be read by this emulator..."), no record.

As with CreateVocabulary, no operation of this service reports a filter's words (GetVocabularyFilter returns name/language/LastModifiedTime/DownloadUri), so recording the filter with the file URI remembered is wire-indistinguishable from the real service while refusing it breaks the whole file-based filter lifecycle.

## Evidence

Spec: `CreateVocabularyFilter` models `VocabularyFilterFileUri` (Uri, len 1..2000) as the documented alternative to `Words` — "You can use Words or VocabularyFilterFileUri, but you can't use both" (quoted in the handler's own doc header). Handler: src/ops/create_vocabulary_filter.rs lines 223-253, `ContentSource::File(uri)` -> `bad_request(...)` before any insert (line 331). Tests: tests/all/vocabularies.rs lines 257-265 ("filter from an s3 file this emulator cannot read") asserts the 400, locking in the divergence; nothing asserts acceptance.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.CreateVocabularyFilter and body {"VocabularyFilterName":"words-from-s3","LanguageCode":"en-US","VocabularyFilterFileUri":"s3://example-bucket/words.txt"}
2. Observe HTTP 400 BadRequestException instead of HTTP 200 with VocabularyFilterName/LanguageCode/LastModifiedTime
3. Call Transcribe.GetVocabularyFilter for the same name — not found, where AWS returns the filter
