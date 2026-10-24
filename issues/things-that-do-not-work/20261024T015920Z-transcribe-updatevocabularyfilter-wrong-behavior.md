Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: ab8204cef26f0e4f4f70841aaff540c660da88f2

# UpdateVocabularyFilter refuses a VocabularyFilterFileUri update with 400 instead of rebuilding the filter

_Filed: 2026-10-24T01:59:20Z_
_Operation: UpdateVocabularyFilter | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `ab8204cef26f0e4f4f70841aaff540c660da88f2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

UpdateVocabularyFilter with `VocabularyFilterFileUri` (and no `Words`) is refused with HTTP 400 BadRequestException and the stored filter is left untouched. The real service accepts it, overwrites the filter's words and answers HTTP 200 with `{VocabularyFilterName, LanguageCode, LastModifiedTime}`.

Expected: 200 and an advanced LastModifiedTime on the stored filter.
Actual: 400 BadRequestException ("The VocabularyFilterFileUri <uri> can't be read by this emulator..."), no state change, so a caller that re-uploads a word-list file cannot observe the update.

## Evidence

Spec: `UpdateVocabularyFilter` models `VocabularyFilterFileUri` (Uri, len 1..2000) as the documented alternative to `Words` (smithy_query operation=UpdateVocabularyFilter; "You can use Words or VocabularyFilterFileUri, but you can't use both", quoted in the handler header). Handler: src/ops/update_vocabulary_filter.rs lines 201-229, `ContentSource::File(uri)` -> `bad_request(...)`, returning before the record mutation at lines 260-264. No output shape of this service reports a filter's words, so the file branch can be recorded rather than refused. No test asserts acceptance.

## Steps to Reproduce

1. CreateVocabularyFilter with inline Words
2. POST / with X-Amz-Target: Transcribe.UpdateVocabularyFilter and body {"VocabularyFilterName":"<name>","VocabularyFilterFileUri":"s3://example-bucket/words.txt"}
3. Observe HTTP 400 BadRequestException instead of HTTP 200
4. Call GetVocabularyFilter — LastModifiedTime is unchanged, where AWS reports the update
