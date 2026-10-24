Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: a32383ef1b0ca99666925af11391d8042f80bab1

# DeleteVocabularyFilter answers an empty 200 for a filter the account does not hold instead of the modelled not-found failure

_Filed: 2026-10-24T10:53:35Z_
_Operation: DeleteVocabularyFilter | Class: error-response_

> _Note: this issue was identified against the code base at commit `a32383ef1b0ca99666925af11391d8042f80bab1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

The handler removes the key and discards whether anything was there (`let removed = store.vocabulary_filters.remove(..).is_some();` — `removed` is only logged), then always answers this operation's empty success. A name the account holds no custom vocabulary filter under therefore gets HTTP 200.

The model declares `NotFoundException` (HTTP 404) and `BadRequestException` (HTTP 400) on `DeleteVocabularyFilter`, and `BadRequestException`'s own documentation covers "the entity you're trying to delete doesn't exist" — so the real service has a not-found failure mode for this operation and answers it ("The requested vocabulary filter couldn't be found..."). The emulator reports success for a resource that never existed, which hides the distinction a caller uses to decide whether its create ever took.

The same crate refuses an absent resource on `DeleteTranscriptionJob`, `DeleteMedicalScribeJob` and `DeleteCallAnalyticsCategory` with exactly the `BadRequestException` sentence quoted above, and those operations do not even model `NotFoundException` while this one does.

Expected: a 4xx modelled client error with nothing changed. Actual: HTTP 200, empty body.

## Evidence

Spec: `smithy_query(query="operation", operation="DeleteVocabularyFilter")` lists BadRequestException (400), InternalFailureException (500), LimitExceededException (429), NotFoundException (404); `BadRequestException` doc: "This can occur when the entity you're trying to delete doesn't exist or if it's in a non-terminal state (such as IN PROGRESS)." Handler: services/service-com-amazonaws-transcribe-transcribe/src/ops/delete_vocabulary_filter.rs lines 183-204 — the lookup result is only used for a tracing field and the handler returns `Ok(DeleteVocabularyFilterOutput)` unconditionally. Tests: tests/all/vocabulary_delete_and_tag_sets.rs lines 150-191 and 196-228 run the absent-delete and repeated-delete axes across both the vocabulary and the filter family and assert the empty success, so the current behaviour is pinned by tests rather than merely untested.

## Steps to Reproduce

1. POST / with X-Amz-Target: Transcribe.DeleteVocabularyFilter and body {"VocabularyFilterName":"never-created"} on a fresh emulator
2. Observe HTTP 200 with an empty body; AWS answers a 4xx modelled client error for a filter it does not hold
3. Create a filter with Words, delete it (200), then delete the same name again: the second call also answers 200 where AWS reports the filter as absent
